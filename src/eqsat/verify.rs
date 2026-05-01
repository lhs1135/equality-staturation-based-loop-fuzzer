use std::process::Command;

/// Result of an alive-tv verification run.
#[derive(Debug)]
pub enum VerifyResult {
    Verified,
    Rejected { reason: String },
    Error(String),
}

/// Call `alive-tv <source_path> <target_path>`, parse the result, and return
/// both the verdict and the full raw output (stdout + stderr) for logging.
pub fn alive2_verify(
    alive_tv: &str,
    source_path: &str,
    target_path: &str,
) -> (VerifyResult, String) {
    let output = match Command::new(alive_tv)
        .arg(source_path)
        .arg(target_path)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            let msg = format!("cannot run '{}': {}", alive_tv, e);
            return (VerifyResult::Error(msg.clone()), msg);
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    let verified = combined
        .lines()
        .filter(|l| l.contains("Transformation seems to be correct!"))
        .count();

    let errors: Vec<&str> = combined
        .lines()
        .filter(|l| l.starts_with("ERROR"))
        .collect();

    let result = if !errors.is_empty() {
        VerifyResult::Rejected { reason: errors.join("; ") }
    } else if verified > 0 {
        VerifyResult::Verified
    } else {
        VerifyResult::Rejected {
            reason: format!("no functions verified; output: {}", combined.trim()),
        }
    };

    (result, combined)
}
