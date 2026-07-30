//! The report.
//!
//! Two audiences. A maintainer wants to know which step of which case broke and why, without
//! opening the specification. A skeptic reading someone's published conformance run wants the case
//! list, the invariants each case covers, and a total they can compare against §18. Both get the
//! same output, because a conformance claim that is only reproducible by the vendor is not a claim.

use crate::runner::CaseResult;

/// Print the per-case report and the summary, returning true when the run counts as a pass.
pub fn print(results: &[CaseResult], level: u8) -> bool {
    let considered: Vec<&CaseResult> = results.iter().filter(|r| r.level <= level).collect();
    let passed = considered.iter().filter(|r| r.passed()).count();
    let total = considered.len();

    for result in &considered {
        let mark = if result.passed() { "PASS" } else { "FAIL" };
        let invariants = if result.invariants.is_empty() {
            String::new()
        } else {
            format!("  [{}]", result.invariants.join(" "))
        };
        println!("{mark}  {:<6} {}{invariants}", result.id, result.title);
        if let Some(failure) = &result.failure {
            println!("      step: {}", failure.step);
            for line in failure.reason.lines() {
                println!("      {line}");
            }
        }
    }

    println!();
    println!("{passed}/{total} passing");

    let failed_level_one: Vec<&str> = considered
        .iter()
        .filter(|r| r.level == 1 && !r.passed())
        .map(|r| r.id.as_str())
        .collect();
    if !failed_level_one.is_empty() {
        println!(
            "not conformant: {} Level 1 case(s) failing — {}",
            failed_level_one.len(),
            failed_level_one.join(", ")
        );
    }
    failed_level_one.is_empty()
}
