use gen_audio_mcp::http;
use gen_audio_mcp::Server;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help") {
        eprintln!("gen-audio-mcp [--smoke] [--http 127.0.0.1:8765]");
        std::process::exit(0);
    }
    if args.iter().any(|arg| arg == "--smoke") {
        match gen_audio_mcp::smoke() {
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
    if let Some(index) = args.iter().position(|arg| arg == "--http") {
        let Some(addr) = args.get(index + 1) else {
            eprintln!("error: --http needs an address");
            std::process::exit(2);
        };
        if let Err(err) = http::serve(addr) {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
        return;
    }
    let server = Server::boot();
    eprintln!(
        "gen-audio-mcp stdio (stateless). work directory: {}",
        server.work.display()
    );
    gen_audio_mcp::stdio_loop(&server);
}
