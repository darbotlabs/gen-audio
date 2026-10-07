use gen_audio_acp::Agent;

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--smoke");
    if smoke {
        match gen_audio_acp::smoke() {
            Ok(message) => {
                println!("{message}");
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("error: {err}");
                std::process::exit(1);
            }
        }
    }
    if std::env::args().any(|arg| arg == "--help") {
        eprintln!("gen-audio-acp [--smoke]");
        eprintln!("stdio ACP agent. Sessions follow the ACP spec; this is not the stateless MCP server.");
        std::process::exit(0);
    }
    gen_audio_acp::stdio_loop(&Agent::new());
}
