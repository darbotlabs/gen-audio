use gen_audio_harness::{load_repo_script, run, trace_to_jsonl, HarnessOptions};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help") {
        eprintln!("gen-audio-harness [--fixture] [--improve] [--script-file PATH]");
        eprintln!("Prints a JSONL trace. Does not invent podcast audio.");
        std::process::exit(0);
    }
    let script = if let Some(index) = args.iter().position(|arg| arg == "--script-file") {
        let Some(path) = args.get(index + 1) else {
            eprintln!("error: --script-file needs a path");
            std::process::exit(2);
        };
        match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("error: {err}");
                std::process::exit(1);
            }
        }
    } else {
        match load_repo_script() {
            Ok(text) => text,
            Err(err) => {
                eprintln!("error: {err}");
                std::process::exit(1);
            }
        }
    };
    let options = HarnessOptions {
        script_text: script,
        cast_engine: "kokoro_onnx".into(),
        write_fixture: args.iter().any(|arg| arg == "--fixture"),
        run_improve: args.iter().any(|arg| arg == "--improve"),
    };
    match run(&options) {
        Ok(trace) => {
            println!("{}", trace_to_jsonl(&trace));
        }
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
