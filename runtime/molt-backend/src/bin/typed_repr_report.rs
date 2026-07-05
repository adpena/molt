#[path = "typed_repr_report/input.rs"]
mod input;
#[path = "typed_repr_report/json_report.rs"]
mod json_report;
#[path = "typed_repr_report/names.rs"]
mod names;
#[path = "typed_repr_report/pipeline.rs"]
mod pipeline;
#[path = "typed_repr_report/stats.rs"]
mod stats;
#[cfg(test)]
#[path = "typed_repr_report/tests.rs"]
mod tests;

use pipeline::run;

fn main() {
    let outcome = run();
    match outcome {
        Ok((payload, verified)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).expect("report JSON must serialize")
            );
            if !verified {
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("typed_repr_report: {err}");
            std::process::exit(2);
        }
    }
}
