mod eqsat;
mod exec;

use rand::Rng;
use rayon::prelude::*;
use std::collections::HashMap;
use std::env;
use std::process::Command;

#[derive(Debug, Clone)]
struct Block {
    label: String,
    phi_lines: Vec<usize>,   // line indices of PHI instructions
    terminator_line: usize,  // usize::MAX = not found
    successors: Vec<String>, // direct successor labels (from terminator)
}

// ---------------------------------------------------------------------------
// IR generation
// ---------------------------------------------------------------------------

fn generate_ir(llvm_stress: &str, seed: u32) -> Result<String, String> {
    let output = Command::new(llvm_stress)
        .arg(format!("--seed={}", seed))
        .output()
        .map_err(|e| format!("Cannot run '{}': {}", llvm_stress, e))?;
    if !output.status.success() {
        return Err(format!(
            "llvm-stress failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// IR parsing
// ---------------------------------------------------------------------------

/// Extract successor block labels from a `br` terminator on a single line.
fn parse_branch_targets(line: &str) -> Vec<String> {
    let s = line.trim();
    if !s.starts_with("br ") {
        return vec![];
    }
    let mut targets = Vec::new();
    let needle = "label %";
    let mut pos = 0;
    while let Some(rel) = s[pos..].find(needle) {
        let start = pos + rel + needle.len();
        let end = s[start..]
            .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '.' && c != '-')
            .map(|j| start + j)
            .unwrap_or(s.len());
        if end > start {
            targets.push(s[start..end].to_string());
        }
        pos = start;
    }
    targets
}

fn is_terminator(s: &str) -> bool {
    matches!(
        s.trim().split_whitespace().next().unwrap_or(""),
        "br" | "ret" | "switch" | "unreachable" | "indirectbr" | "invoke" | "resume" | "callbr"
    )
}

/// Parse all basic blocks in the first function found in `lines`.
///
/// llvm-stress format:
///   define ... {
///   BB:
///     <instructions>
///     <terminator>
///   BBN:           ; preds = ...
///     ...
///   }
fn parse_blocks(lines: &[&str]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut in_function = false;

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if !in_function {
            if trimmed.starts_with("define ") {
                in_function = true;
            }
            continue;
        }

        if trimmed == "}" {
            in_function = false;
            continue;
        }

        // Block label: no leading whitespace, not a comment, not `{`/`}`
        let is_label_line = !line.starts_with(' ')
            && !line.starts_with('\t')
            && !trimmed.is_empty()
            && !trimmed.starts_with(';')
            && !trimmed.starts_with('{');

        if is_label_line {
            if let Some(colon_pos) = trimmed.find(':') {
                let label_part = &trimmed[..colon_pos];
                let after = trimmed[colon_pos + 1..].trim();
                let valid = !label_part.is_empty()
                    && !label_part.contains(' ')
                    && label_part
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
                    && (after.is_empty() || after.starts_with(';'));
                if valid {
                    blocks.push(Block {
                        label: label_part.to_string(),
                        phi_lines: Vec::new(),
                        terminator_line: usize::MAX,
                        successors: Vec::new(),
                    });
                    continue;
                }
            }
        }

        if let Some(block) = blocks.last_mut() {
            if !trimmed.is_empty() && !trimmed.starts_with(';') {
                if trimmed.contains("= phi ") {
                    block.phi_lines.push(i);
                }
                if is_terminator(trimmed) {
                    block.terminator_line = i;
                    block.successors = parse_branch_targets(trimmed);
                }
            }
        }
    }

    blocks
}

// ---------------------------------------------------------------------------
// Back-edge insertion
// ---------------------------------------------------------------------------

/// Add a random back-edge B → P to create a CFG cycle (loop).
///
/// Strategy
/// --------
/// Pick a block B whose terminator is an unconditional branch `br label %X`
/// and which has a direct CFG predecessor P (P → B exists, P ≠ entry, P ≠ B).
/// Insert before B's terminator:
///
///   %backedge_N = freeze i1 poison
///   br i1 %backedge_N, label %P, label %X
///
/// The original B → X edge is preserved (via the false branch), so no PHI
/// nodes in X need updating.  Only P's PHI nodes gain a new arm `[ undef, %B ]`.
///
/// If no suitable block exists in this IR, returns None — caller should skip
/// and try the next llvm-stress output.
fn add_back_edge(ir: &str, rng: &mut impl Rng) -> Option<(String, String, String)> {
    let original_lines: Vec<&str> = ir.lines().collect();
    let blocks = parse_blocks(&original_lines);

    if blocks.len() < 2 {
        return None;
    }

    // pred_map[X] = all blocks that directly branch to X
    let mut pred_map: HashMap<String, Vec<String>> = HashMap::new();
    for b in &blocks {
        for succ in &b.successors {
            pred_map.entry(succ.clone()).or_default().push(b.label.clone());
        }
    }

    let entry_label = &blocks[0].label;

    // Candidates: B has exactly 1 successor (unconditional branch only),
    // and a direct predecessor P that is not the entry block and not B itself.
    let mut candidates: Vec<(&Block, String)> = Vec::new();
    for b in &blocks {
        if b.terminator_line == usize::MAX || b.successors.len() != 1 {
            continue;
        }
        let Some(preds) = pred_map.get(&b.label) else {
            continue;
        };
        for p in preds {
            if p != entry_label && p != &b.label {
                candidates.push((b, p.clone()));
            }
        }
    }

    if candidates.is_empty() {
        return None;
    }

    let (b_block, p_label) = &candidates[rng.gen_range(0..candidates.len())];
    let p_block = blocks.iter().find(|bl| &bl.label == p_label)?;

    let mut new_lines: Vec<String> = original_lines.iter().map(|l| l.to_string()).collect();

    // 1. Add B's PHI arm to P (undef is valid for a new back-edge).
    for &phi_idx in &p_block.phi_lines {
        let existing = new_lines[phi_idx].trim_end().to_string();
        new_lines[phi_idx] = format!("{}, [ undef, %{} ]", existing, b_block.label);
    }

    // 2. Replace B's unconditional branch with a conditional one.
    //    Insert the freeze condition just before the (now-replaced) terminator.
    let term_idx = b_block.terminator_line;
    let indent: String = new_lines[term_idx]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let cond_var = format!("backedge_{}", term_idx);
    let orig_succ = b_block.successors[0].clone();

    new_lines[term_idx] = format!(
        "{}br i1 %{}, label %{}, label %{}",
        indent, cond_var, p_label, orig_succ
    );
    new_lines.insert(
        term_idx,
        format!("{}%{} = freeze i1 poison", indent, cond_var),
    );

    Some((new_lines.join("\n"), b_block.label.clone(), p_label.clone()))
}

// ---------------------------------------------------------------------------
// Loop fusion via opt
// ---------------------------------------------------------------------------

/// Strip LLVM IR comments (`;` to end-of-line) so that cosmetic changes like
/// `; preds = ...` reorderings do not count as semantic differences.
fn strip_comments(ir: &str) -> String {
    ir.lines()
        .map(|line| {
            if let Some(pos) = line.find(';') {
                line[..pos].trim_end().to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run opt in two stages to isolate loop-fusion changes from preprocessing.
///
/// Stage 1: loop-simplify + lcssa  →  normalized form required by loop-fusion.
/// Stage 2: loop-fusion only       →  `output_path` (the fused IR).
///
/// Returns Ok(true) only when loop-fusion itself made a semantic change
/// (comments such as `; preds = …` reorderings are ignored).
///
/// Also writes the stage-1 normalized IR to `norm_path` for reference.
fn run_opt_fusion(
    opt: &str,
    input_path: &str,
    output_path: &str,
    norm_path: &str,
) -> Result<bool, String> {
    // Stage 1: normalize into LCSSA / loop-simplified form.
    let s1 = Command::new(opt)
        .args(["-passes=loop-simplify,lcssa", "-S", input_path, "-o", norm_path])
        .status()
        .map_err(|e| format!("Cannot run '{}': {}", opt, e))?;
    if !s1.success() {
        return Err(format!("opt (normalize) exited with: {}", s1));
    }

    // Stage 2: loop-fusion only, starting from the normalized IR.
    let s2 = Command::new(opt)
        .args(["-passes=loop-fusion", "-S", norm_path, "-o", output_path])
        .status()
        .map_err(|e| format!("Cannot run '{}': {}", opt, e))?;
    if !s2.success() {
        return Err(format!("opt (loop-fusion) exited with: {}", s2));
    }

    // Compare stripping comments so that "; preds = ..." reorderings are ignored.
    let before = std::fs::read_to_string(norm_path)
        .map(|s| strip_comments(&s))
        .unwrap_or_default();
    let after = std::fs::read_to_string(output_path)
        .map(|s| strip_comments(&s))
        .unwrap_or_default();
    Ok(before != after)
}

/// Return the unified diff of two files using the system `diff` command.
fn unified_diff(a_path: &str, b_path: &str) -> String {
    match Command::new("diff").args(["-u", a_path, b_path]).output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
        Err(e) => format!("diff unavailable: {}", e),
    }
}

// ---------------------------------------------------------------------------
// Output organisation
// ---------------------------------------------------------------------------

/// Move all files belonging to case `idx` from `out_dir` into `out_dir/subdir/`.
/// Files that do not exist (e.g. bins already deleted) are silently skipped.
fn move_case_to(out_dir: &str, idx: usize, subdir: &str) {
    let target = format!("{out_dir}/{subdir}");
    std::fs::create_dir_all(&target).unwrap_or(());
    let candidates = [
        format!("orig_{idx:04}.ll"),
        format!("loop_{idx:04}.ll"),
        format!("norm_{idx:04}.ll"),
        format!("fused_{idx:04}.ll"),
        format!("diff_{idx:04}.txt"),
        format!("verify_{idx:04}.txt"),
        format!("norm_{idx:04}_bin"),
        format!("fused_{idx:04}_bin"),
        format!("exec_{idx:04}.txt"),
    ];
    for name in &candidates {
        let _ = std::fs::rename(
            format!("{out_dir}/{name}"),
            format!("{target}/{name}"),
        );
    }
}

// ---------------------------------------------------------------------------
// Per-iteration outcome
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum IterOutcome {
    Skipped,            // no valid back-edge found after retries
    NoFusion,           // opt ran but fusion did not fire
    OptError,           // opt itself failed
    FusionHit,          // fusion fired; alive2 not configured
    VerificationFailed, // alive2 rejected or errored
    VerifiedNoExec,     // alive2 OK; clang not configured
    ExecClean,          // alive2 OK + exec outputs match
    ExecError,          // alive2 OK + exec infrastructure error
    BugFound,           // alive2 OK + exec outputs differ
}

/// Run one fuzzing iteration identified by `done` (used as the file-name index).
/// Each call gets its own thread-local RNG so threads are fully independent.
/// Returns an optional log line: None means nothing noteworthy happened (no fusion).
fn run_iteration(
    done: usize,
    out_dir: &str,
    llvm_stress: &str,
    opt_path: Option<&str>,
    alive_tv_path: Option<&str>,
    clang_path: Option<&str>,
) -> (IterOutcome, Option<String>) {
    let mut rng = rand::thread_rng();

    // Try up to 30 seeds to find one that admits a back-edge.
    let (ir, modified, b_label, p_label) = {
        let mut found = None;
        for _ in 0..30 {
            let seed: u32 = rng.gen();
            let ir = match generate_ir(llvm_stress, seed) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if let Some((m, b, p)) = add_back_edge(&ir, &mut rng) {
                found = Some((ir, m, b, p));
                break;
            }
        }
        match found {
            None => return (IterOutcome::Skipped, None),
            Some(v) => v,
        }
    };

    let orig_path = format!("{out_dir}/orig_{done:04}.ll");
    let loop_path = format!("{out_dir}/loop_{done:04}.ll");
    std::fs::write(&orig_path, &ir).expect("write failed");
    std::fs::write(&loop_path, &modified).expect("write failed");

    let prefix = format!("[{done:3}] back-edge {b_label} → {p_label}");

    let Some(opt) = opt_path else {
        return (IterOutcome::FusionHit, Some(prefix));
    };

    let norm_path  = format!("{out_dir}/norm_{done:04}.ll");
    let fused_path = format!("{out_dir}/fused_{done:04}.ll");
    let diff_path  = format!("{out_dir}/diff_{done:04}.txt");

    match run_opt_fusion(opt, &loop_path, &fused_path, &norm_path) {
        Err(e) => {
            let _ = std::fs::remove_file(&norm_path);
            return (IterOutcome::OptError, Some(format!("{prefix}  |  opt error: {e}")));
        }
        Ok(false) => {
            let _ = std::fs::remove_file(&fused_path);
            let _ = std::fs::remove_file(&norm_path);
            let _ = std::fs::remove_file(&loop_path);
            let _ = std::fs::remove_file(&orig_path);
            return (IterOutcome::NoFusion, None);
        }
        Ok(true) => {}
    }

    let diff = unified_diff(&norm_path, &fused_path);
    std::fs::write(&diff_path, &diff).expect("write diff failed");

    let Some(alive_tv) = alive_tv_path else {
        return (
            IterOutcome::FusionHit,
            Some(format!("{prefix}  |  FUSION FIRED  →  {fused_path}  diff: {diff_path}")),
        );
    };

    use eqsat::verify::{alive2_verify, VerifyResult};
    let verify_path = format!("{out_dir}/verify_{done:04}.txt");
    let (verify_result, raw) = alive2_verify(alive_tv, &norm_path, &fused_path);
    std::fs::write(&verify_path, &raw).expect("write verify failed");

    match verify_result {
        VerifyResult::Rejected { reason } => {
            move_case_to(out_dir, done, "verification_failed");
            return (
                IterOutcome::VerificationFailed,
                Some(format!("{prefix}  |  alive2 rejected: {reason}  →  {out_dir}/verification_failed/")),
            );
        }
        VerifyResult::Error(e) => {
            move_case_to(out_dir, done, "verification_failed");
            return (
                IterOutcome::VerificationFailed,
                Some(format!("{prefix}  |  alive2 error: {e}  →  {out_dir}/verification_failed/")),
            );
        }
        VerifyResult::Verified => {}
    }

    let verified_prefix = format!("{prefix}  |  FUSION FIRED + VERIFIED");

    let Some(clang) = clang_path else {
        move_case_to(out_dir, done, "verified");
        return (
            IterOutcome::VerifiedNoExec,
            Some(format!("{verified_prefix}  →  {out_dir}/verified/")),
        );
    };

    let norm_bin  = format!("{out_dir}/norm_{done:04}_bin");
    let fused_bin = format!("{out_dir}/fused_{done:04}_bin");
    let exec_path = format!("{out_dir}/exec_{done:04}.txt");

    match exec::differential_test(clang, &norm_path, &fused_path, &norm_bin, &fused_bin) {
        exec::ExecResult::Match => {
            let _ = std::fs::remove_file(&norm_bin);
            let _ = std::fs::remove_file(&fused_bin);
            move_case_to(out_dir, done, "verified");
            (
                IterOutcome::ExecClean,
                Some(format!("{verified_prefix}  |  exec: match  →  {out_dir}/verified/")),
            )
        }
        exec::ExecResult::Mismatch { norm_out, fused_out } => {
            let content = format!(
                "=== norm output ===\n{norm_out}\n\
                 === fused output ===\n{fused_out}\n"
            );
            std::fs::write(&exec_path, &content).expect("write exec result failed");
            move_case_to(out_dir, done, "buggy");
            (
                IterOutcome::BugFound,
                Some(format!("{verified_prefix}  |  BUG FOUND  →  {out_dir}/buggy/")),
            )
        }
        exec::ExecResult::Error(e) => {
            let _ = std::fs::remove_file(&norm_bin);
            let _ = std::fs::remove_file(&fused_bin);
            move_case_to(out_dir, done, "exec_failed");
            (
                IterOutcome::ExecError,
                Some(format!("{verified_prefix}  |  exec error: {e}  →  {out_dir}/exec_failed/")),
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Eqsat demo
// ---------------------------------------------------------------------------

fn run_eqsat_demo() {
    println!("=== Equality Saturation Demo (egg) ===\n");

    let examples: &[(&str, &str)] = &[
        // LICM: hoist invariant head/tail out of the loop so adjacent loops
        // are exposed for LLVM's loop-fusion pass.
        ("licm hoist head",  "(loop 0 100 1 (seq K body))"),
        ("licm hoist tail",  "(loop 0 100 1 (seq body K))"),
        // After LICM the two loops become adjacent — LLVM can then fuse them.
        (
            "licm exposes adjacent loops",
            "(seq (loop 0 N 1 (seq body1 K)) \
                  (loop 0 N 1 (seq K body2)))",
        ),
        ("add-zero simplification", "(+ x 0)"),
        ("mul-one simplification",  "(* y 1)"),
        ("sub-self simplification", "(- z z)"),
        ("seq-nop elimination",     "(seq nop (+ i 1))"),
        ("dead loop elimination",   "(loop 0 10 1 nop)"),
    ];

    for (name, expr) in examples {
        let best  = eqsat::optimize(expr);
        let worst = eqsat::optimize_worst(expr);
        match (best, worst) {
            (Ok((bc, br)), Ok((wc, wr))) =>
                println!("[{name}]\n  in   : {expr}\n  best : {br}  (cost {bc})\n  worst: {wr}  (cost {wc})\n"),
            (Err(e), _) | (_, Err(e)) =>
                println!("[{name}] ERROR: {e}\n"),
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("--help") | Some("-h") => {
            print!(
                "\
Usage: eqsat-loop-fuzz [OPTIONS]

ARGUMENTS (all positional, all optional):
  1  <iters>       Number of loop IR pairs to generate           [default: 10]
  2  <out_dir>     Output directory for all generated files      [default: out]
  3  <llvm-stress> Path to the llvm-stress binary                [default: llvm-stress]
  4  <opt>         Path to opt; enables loop-fusion step         [default: skip]
  5  <alive-tv>    Path to alive-tv; enables alive2 verification [default: skip]
  6  <clang>       Path to clang; enables differential execution [default: skip]

FLAGS:
  --eqsat-demo    Run the equality saturation demo and exit
  -h, --help      Print this help message and exit

OUTPUT FILES (written to <out_dir>/):
  orig_NNNN.ll       Raw llvm-stress output
  loop_NNNN.ll       IR with a synthetic back-edge inserted
  norm_NNNN.ll       Loop-simplified / LCSSA-normalised IR  (requires <opt>)
  fused_NNNN.ll      Loop-fused IR                          (requires <opt>, fusion fired)
  diff_NNNN.txt      Unified diff of norm vs fused          (requires <opt>, fusion fired)
  verify_NNNN.txt    alive-tv stdout+stderr                 (requires <alive-tv>, fusion fired)
  norm_NNNN_bin      Compiled norm binary — kept only on bug (requires <clang>)
  fused_NNNN_bin     Compiled fused binary — kept only on bug (requires <clang>)
  exec_NNNN.txt      Both execution outputs — created only on bug (requires <clang>)

PIPELINE:
  generate IR → add back-edge → loop-fusion (opt) → alive2 verify → diff exec (clang)
  Each stage is skipped when its tool argument is absent.
  Binaries and exec results are kept only when outputs differ (potential bug).

PARALLELISM:
  Iterations run in parallel across all available CPU threads.
  Use RAYON_NUM_THREADS=N to cap concurrency.

EXAMPLES:
  # Generate 10 pairs (IR only):
  eqsat-loop-fuzz

  # Full pipeline — 200 iterations:
  eqsat-loop-fuzz 200 out llvm-stress opt ~/alive2/build/alive-tv clang

  # Full pipeline — 200 iterations, 8 threads:
  RAYON_NUM_THREADS=8 eqsat-loop-fuzz 200 out llvm-stress opt ~/alive2/build/alive-tv clang

  # Run the equality saturation demo:
  eqsat-loop-fuzz --eqsat-demo
"
            );
            return;
        }
        Some("--eqsat-demo") => {
            run_eqsat_demo();
            return;
        }
        _ => {}
    }

    let iterations: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let out_dir = args.get(2).cloned().unwrap_or_else(|| "out".to_string());
    let llvm_stress = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| "llvm-stress".to_string());
    let opt_path       = args.get(4).cloned();
    let alive_tv_path  = args.get(5).cloned();
    let clang_path     = args.get(6).cloned();

    std::fs::create_dir_all(&out_dir).expect("Failed to create output directory");

    if opt_path.is_none() {
        eprintln!(
            "Note: no opt path given — loop-fusion step will be skipped.\n\
             Usage: eqsat-loop-fuzz <iters> <out_dir> <llvm-stress> <opt> [alive-tv] [clang]"
        );
    }
    if alive_tv_path.is_some() {
        eprintln!("Note: alive2 verification enabled — only verified fusions will be counted.");
    }
    if clang_path.is_some() {
        eprintln!("Note: differential execution enabled — binaries kept only for buggy cases.");
    }

    let threads = rayon::current_num_threads();
    println!(
        "Generating {} loop IR file pairs into '{out_dir}/'  (threads: {threads}) ...",
        iterations
    );

    // Run all iterations in parallel; each thread owns its RNG.
    let mut outcomes: Vec<(usize, IterOutcome)> = (0..iterations)
        .into_par_iter()
        .map(|done| {
            let (outcome, msg) = run_iteration(
                done,
                &out_dir,
                &llvm_stress,
                opt_path.as_deref(),
                alive_tv_path.as_deref(),
                clang_path.as_deref(),
            );
            if let Some(line) = msg {
                println!("{line}");
            }
            (done, outcome)
        })
        .collect();

    // Sort by index so the summary is ordered.
    outcomes.sort_by_key(|(i, _)| *i);

    // Categorise outcomes for the summary.
    let mut fusion_hits:         Vec<usize> = Vec::new();
    let mut verification_failed: Vec<usize> = Vec::new();
    let mut exec_failed:         Vec<usize> = Vec::new();
    let mut verified_clean:      Vec<usize> = Vec::new();
    let mut exec_bugs:           Vec<usize> = Vec::new();
    let mut skipped = 0usize;

    for (done, outcome) in &outcomes {
        match outcome {
            IterOutcome::Skipped                         => skipped += 1,
            IterOutcome::NoFusion | IterOutcome::OptError => {}
            IterOutcome::FusionHit                       => fusion_hits.push(*done),
            IterOutcome::VerificationFailed              => verification_failed.push(*done),
            IterOutcome::VerifiedNoExec | IterOutcome::ExecClean => {
                fusion_hits.push(*done);
                verified_clean.push(*done);
            }
            IterOutcome::ExecError => {
                fusion_hits.push(*done);
                exec_failed.push(*done);
            }
            IterOutcome::BugFound => {
                fusion_hits.push(*done);
                exec_bugs.push(*done);
            }
        }
    }

    let done_count = iterations - skipped;
    if skipped > 0 {
        eprintln!("Warning: {skipped}/{iterations} iterations skipped (no valid back-edge found).");
    }

    if opt_path.is_some() {
        println!("\n=== eqsat-loop-fuzz summary ===");
        println!("Iterations : {done_count}");
        println!("Fusion hits: {}", fusion_hits.len());
        println!();

        if alive_tv_path.is_some() {
            println!(
                "Verification failed : {}  →  {out_dir}/verification_failed/",
                verification_failed.len()
            );
            for idx in &verification_failed {
                println!("  iter {idx:04}  verify: {out_dir}/verification_failed/verify_{idx:04}.txt");
            }

            if clang_path.is_some() {
                println!();
                println!(
                    "Exec failed         : {}  →  {out_dir}/exec_failed/",
                    exec_failed.len()
                );
                for idx in &exec_failed {
                    println!("  iter {idx:04}  norm: {out_dir}/exec_failed/norm_{idx:04}.ll");
                }

                println!();
                println!(
                    "Verified (clean)    : {}  →  {out_dir}/verified/",
                    verified_clean.len()
                );
                for idx in &verified_clean {
                    println!("  iter {idx:04}  fused: {out_dir}/verified/fused_{idx:04}.ll");
                }

                println!();
                println!(
                    "Bugs found          : {}  →  {out_dir}/buggy/",
                    exec_bugs.len()
                );
                for idx in &exec_bugs {
                    println!(
                        "  iter {idx:04}  result: {out_dir}/buggy/exec_{idx:04}.txt  \
                         norm_bin: {out_dir}/buggy/norm_{idx:04}_bin  \
                         fused_bin: {out_dir}/buggy/fused_{idx:04}_bin"
                    );
                }
            }
        }
    } else {
        println!("\nDone: {done_count} file pairs in '{out_dir}/'.");
    }
}
