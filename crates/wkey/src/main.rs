const USAGE: &str = "usage: wkey [lsp]\n\n  wkey      which-key view of Zed's vim bindings at the cursor\n  wkey lsp  the language server Zed runs to report the cursor";

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("lsp") => wkey::lsp::serve(),
        None => wkey::tui::main(),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2)
        }
    };
    if let Err(e) = result {
        eprintln!("wkey: {e:#}");
        std::process::exit(1)
    }
}
