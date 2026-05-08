use std::fs;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const BUF_SIZE: usize = 4096;
const EXEC_TIMEOUT_MS: u64 = 100;

// llvm-stress always generates this fixed signature:
//   void @autogen_SD<seed>(ptr, ptr, ptr, i32, i64, i8)
// so the harness is a static template — only the function name varies.
const HARNESS_TEMPLATE: &str = r#"
#include <stdio.h>
#include <string.h>
#include <stdint.h>

extern void FUNC_NAME(void* p0, void* p1, void* p2,
                      int32_t a3, int64_t a4, int8_t a5);

static unsigned char buf0[BUF_SIZE];
static unsigned char buf1[BUF_SIZE];
static unsigned char buf2[BUF_SIZE];

int main(void) {
    memset(buf0, 0xAA, sizeof(buf0));
    memset(buf1, 0xBB, sizeof(buf1));
    memset(buf2, 0xCC, sizeof(buf2));
    FUNC_NAME(buf0, buf1, buf2, 42, 100LL, 7);
    for (int i = 0; i < BUF_SIZE; i++) printf("%02x", buf0[i]);
    for (int i = 0; i < BUF_SIZE; i++) printf("%02x", buf1[i]);
    for (int i = 0; i < BUF_SIZE; i++) printf("%02x", buf2[i]);
    printf("\n");
    return 0;
}
"#;

/// Parse the autogen function name from an LLVM IR string.
pub fn parse_func_name(ir: &str) -> Option<String> {
    ir.lines()
        .find(|l| l.starts_with("define ") && l.contains('@'))
        .and_then(|l| {
            let at = l.find('@')? + 1;
            let len = l[at..].find('(')?;
            Some(l[at..at + len].to_string())
        })
}

pub enum ExecResult {
    /// Both binaries produced identical output — no bug.
    Match,
    /// Outputs differ — potential miscompilation bug.
    Mismatch { norm_out: String, fused_out: String },
    /// Compile or execution error.
    Error(String),
}

/// Compile `norm_path` and `fused_path` each with a generated harness,
/// execute both, and compare their stdout.
///
/// `norm_bin` / `fused_bin` are the output executable paths.
/// On `Mismatch` the caller should keep both binaries; on `Match` or
/// `Error` they should be deleted.
pub fn differential_test(
    clang: &str,
    norm_path: &str,
    fused_path: &str,
    norm_bin: &str,
    fused_bin: &str,
) -> ExecResult {
    // Parse function name from the pre-fusion IR.
    let norm_ir = match fs::read_to_string(norm_path) {
        Ok(s) => s,
        Err(e) => return ExecResult::Error(format!("read {norm_path}: {e}")),
    };
    let func_name = match parse_func_name(&norm_ir) {
        Some(n) => n,
        None => return ExecResult::Error(format!("no function found in {norm_path}")),
    };

    // Write harness to a temp file beside the norm binary.
    let harness_path = format!("{norm_bin}.harness.c");
    let harness = HARNESS_TEMPLATE
        .replace("FUNC_NAME", &func_name)
        .replace("BUF_SIZE", &BUF_SIZE.to_string());
    if let Err(e) = fs::write(&harness_path, &harness) {
        return ExecResult::Error(format!("write harness: {e}"));
    }

    // Compile both IRs against the same harness.
    let compile_result = compile(clang, norm_path, &harness_path, norm_bin)
        .and_then(|_| compile(clang, fused_path, &harness_path, fused_bin));
    let _ = fs::remove_file(&harness_path);
    if let Err(e) = compile_result {
        return ExecResult::Error(e);
    }

    // Execute with timeout and collect stdout.
    let norm_out = match run_with_timeout(norm_bin) {
        Ok(s) => s,
        Err(e) => return ExecResult::Error(format!("run norm: {e}")),
    };
    let fused_out = match run_with_timeout(fused_bin) {
        Ok(s) => s,
        Err(e) => return ExecResult::Error(format!("run fused: {e}")),
    };

    if norm_out == fused_out {
        ExecResult::Match
    } else {
        ExecResult::Mismatch { norm_out, fused_out }
    }
}

fn compile(clang: &str, ir_path: &str, harness_path: &str, exe: &str) -> Result<(), String> {
    let status = Command::new(clang)
        .args([ir_path, harness_path, "-O0", "-Wno-override-module", "-o", exe])
        .status()
        .map_err(|e| format!("cannot run '{}': {}", clang, e))?;
    if !status.success() {
        Err(format!("clang compile failed for {ir_path}"))
    } else {
        Ok(())
    }
}

fn run_with_timeout(exe: &str) -> Result<String, String> {
    let exe_owned = exe.to_string();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(Command::new(&exe_owned).output());
    });
    match rx.recv_timeout(Duration::from_millis(EXEC_TIMEOUT_MS)) {
        Ok(Ok(out)) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(Err(e)) => Err(format!("exec error: {e}")),
        Err(_) => Err(format!("timed out after {}ms", EXEC_TIMEOUT_MS)),
    }
}
