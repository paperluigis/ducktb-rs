mod listener;
mod talker;

use listener::{ListenerTxOp, ListenerRxOp};
use talker::{TalkerTxOp, TalkerRxOp};

use std::path::{Path, PathBuf};

use crate::config::Config;

use figment::{Figment, providers::{Format, Toml}};
use tokio::{select, spawn};
use tokio::sync::mpsc::{Sender, Receiver, channel};

pub struct Server {
	config: Config,
	config_path: PathBuf,

	listener_tx: Sender<listener::ListenerRxOp>,
	listener_rx: Receiver<listener::ListenerTxOp>,

	talker_tx: Sender<talker::TalkerRxOp>,
	talker_rx: Receiver<talker::TalkerTxOp>,
}

fn parse_config(config_path: &Path) -> Result<Config, figment::Error> {
	Figment::new()
		.merge(Toml::file(config_path))
		.extract()
}

impl Server {
	fn reload_config(&mut self) -> Result<(), ()> {
		// TODO: config reload
		let mut _new_config: Config = parse_config(&self.config_path).map_err(|err| {
			error!("failed to parse config: {}", err);
			()
		})?;
		Ok(())
	}

	pub async fn run(config_path: PathBuf) -> Result<(), ()> {
		let config = parse_config(&config_path).map_err(|err| {
			error!("failed to parse config: {}", err);
			()
		})?;
		debug!("config = {:#?}", config);

		// pretty arbitrary bounds
		let (listener_c2s_tx, listener_c2s_rx) = channel::<listener::ListenerTxOp>(256);
		let (listener_s2c_tx, listener_s2c_rx) = channel::<listener::ListenerRxOp>(256);

		let (talker_s2c_tx, talker_s2c_rx) = channel::<talker::TalkerTxOp>(256);
		let (talker_c2s_tx, talker_c2s_rx) = channel::<talker::TalkerRxOp>(256);

		let mut s = Server {
			config,
			config_path,
			listener_tx: listener_s2c_tx,
			listener_rx: listener_c2s_rx,

			talker_tx: talker_c2s_tx,
			talker_rx: talker_s2c_rx,
		};

		let mut listener_jh = spawn(listener::Listener::run(s.config.listener.clone(), listener_c2s_tx, listener_s2c_rx));
		let mut talker_jh = spawn(talker::Talker::run(s.config.talker.clone(), talker_s2c_tx, talker_c2s_rx));

		// right now we just pass messages between those two but sometime we may
		// also add something else..?
		loop {
			select! {
				biased;
				_ = &mut listener_jh => { break },
				_ = &mut talker_jh => { break },
				msg = s.talker_rx.recv() => {
					if let None = msg { break }
					let msg = msg.unwrap();
					debug!("msg talker   {:?}", msg);
					match msg {
						TalkerTxOp::Event(sid, msg) => {
							let _ = s.listener_tx.send(ListenerRxOp::Event(sid, msg)).await;
						}
					}
				},
				msg = s.listener_rx.recv() => {
					if let None = msg { break }
					let msg = msg.unwrap();
					debug!("msg listener {:?}", msg);
					match msg {
						ListenerTxOp::Connect(sid, addr) => {
							let _ = s.talker_tx.send(TalkerRxOp::Connect(sid, addr)).await;
						}
						ListenerTxOp::Disconnect(sid) => {
							let _ = s.talker_tx.send(TalkerRxOp::Disconnect(sid)).await;
						}
						ListenerTxOp::Request(sid, rid, evt) => {
							let _ = s.talker_tx.send(TalkerRxOp::Request(sid, rid, evt)).await;
						}
					}
				},
			};
		}
		warn!("shutting down");
		// tell them to shut down
		let _ = s.listener_tx.send(ListenerRxOp::Shutdown("server exited".into())).await;
		let _ = s.talker_tx.send(TalkerRxOp::Shutdown).await;
		// wait for them to shut down
		if !talker_jh.is_finished() { let _ = talker_jh.await; }
		if !listener_jh.is_finished() { let _ = listener_jh.await; }

		Ok(())
	}
}


