#[macro_use]
mod types;
mod config;
mod server;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use pretty_env_logger;

#[macro_use]
extern crate log;

#[derive(Parser, Debug)]
#[clap(name="ducktb-sv1", about="a ducktb server", version)]
struct Args {
	/// make the server listen on this address and port
	//#[arg(short='b', long="bind", value_name="addr", default_value="127.0.0.1:8000")]
	//bind: SocketAddr,

	/// configure log level (refer to `env_logger` crate documentation)
	#[arg(short='l', long="log", value_name="level", default_value="info")]
	log_level: String,
	/// config path
	#[arg(short='c', long="config", value_name="path", default_value="./config.toml")]
	config_path: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
	let args = Args::parse();

	let mut logger_builder = pretty_env_logger::formatted_builder();
	logger_builder.parse_filters(&args.log_level);
	logger_builder.init();

	match server::Server::run(args.config_path.clone()).await {
		Ok(()) => ExitCode::SUCCESS,
		Err(()) => ExitCode::FAILURE,
	}
}
