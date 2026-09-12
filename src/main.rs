mod cli;

fn main() {
    let action = match cli::parse(std::env::args_os().skip(1).collect()) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("{error}\nUse --help for usage.");
            std::process::exit(2);
        }
    };
    if matches!(action, cli::Action::Help) {
        print!("{}", cli::HELP);
        return;
    }
    let runtime = tokio::runtime::Runtime::new().expect("Hub runtime unavailable");
    if let Err(error) = runtime.block_on(cli::run(action)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
