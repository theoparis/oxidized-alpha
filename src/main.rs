#![deny(warnings)]
use std::{
	io::{Read, Write},
	net::{TcpListener, TcpStream},
	sync::{
		atomic::{AtomicI32, Ordering},
		Arc, Mutex,
	},
};

use irox_bits::{Bits, MutBits};
use oxidized_alpha::{packets, Player};
use snafu::{ensure, Snafu};

pub type Result<T, E = Error> = core::result::Result<T, E>;

#[derive(Debug, Snafu)]
pub enum Error {
	BitsError { message: String },
	WriteHandshakePacket,
	WriteKeepAlivePacket,
	ReadOnGroundPacket,
	ListenFailed,
	InvalidProtocolVersion,
}

impl From<irox_bits::Error> for Error {
	fn from(value: irox_bits::Error) -> Self {
		Self::BitsError {
			message: value.to_string(),
		}
	}
}

pub struct PacketSerializer<'a, Handle: Read + Write> {
	pub handle: &'a mut Handle,
}

impl<Handle: Read + Write> Bits for PacketSerializer<'_, Handle> {
	fn next_u8(&mut self) -> Result<Option<u8>, irox_bits::Error> {
		let mut buffer = [0u8; 1];
		self.handle.read_exact(&mut buffer).map_err(|_| {
			irox_bits::Error::new(
				irox_bits::BitsErrorKind::UnexpectedEof,
				"Failed to read byte",
			)
		})?;

		Ok(Some(buffer[0]))
	}
}

impl<Handle: Read + Write> MutBits for PacketSerializer<'_, Handle> {
	fn write_u8(&mut self, val: u8) -> Result<(), irox_bits::Error> {
		self.handle.write_all(&val.to_be_bytes()).map_err(|_| {
			irox_bits::Error::new(
				irox_bits::BitsErrorKind::UnexpectedEof,
				"Failed to write byte",
			)
		})?;
		self.handle.flush().map_err(|_| {
			irox_bits::Error::new(
				irox_bits::BitsErrorKind::BrokenPipe,
				"Failed to flush stream",
			)
		})?;

		Ok(())
	}
}

impl<'a, Handle: Read + Write> PacketSerializer<'a, Handle> {
	pub fn new(handle: &'a mut Handle) -> Self {
		Self { handle }
	}

	pub fn read_string(&mut self) -> Result<String, irox_bits::Error> {
		let length = self.read_be_u16()?;

		self.read_str_sized_lossy(length as usize)
	}

	pub fn write_chunk(&mut self) -> Result<(), irox_bits::Error> {
		// Write chunk header packet

		Ok(())
	}
}

static ENTITY_COUNTER: AtomicI32 = AtomicI32::new(1);

fn handle_client(
	stream: &mut PacketSerializer<TcpStream>,
	players: &mut Vec<Player>,
) -> Result<()> {
	loop {
		if let Ok(packet_id) = stream.read_u8() {
			match packet_id {
				packets::KEEP_ALIVE => {
					ensure!(
						stream.write_u8(0).is_ok(),
						WriteKeepAlivePacketSnafu
					);
				}
				packets::LOGIN => {
					// Read login request packet
					// TODO: use irox-bits
					let protocol_version = stream.read_be_u32()?;

					tracing::debug!("protocol version: {}", protocol_version);
					ensure!(protocol_version == 3, InvalidProtocolVersionSnafu);

					let username = stream.read_string()?;
					let _password = stream.read_string()?;

					let map_seed = stream.read_be_u64()?;

					tracing::debug!("map seed: {}", map_seed);

					let dimension = stream.read_u8()?;
					tracing::debug!("dimension: {}", dimension);

					let entity_id =
						ENTITY_COUNTER.fetch_add(1, Ordering::Relaxed);

					// Send login response
					stream.write_be_i32(entity_id)?;

					// Two unused/empty strings
					stream.write_be_u16(0)?;
					stream.write_be_u16(0)?;

					stream.write_be_u64(map_seed)?;

					players.push(Player {
						username,
						logged_in: true,
					});
				}
				packets::HANDSHAKE => {
					let username = stream.read_string()?;
					ensure!(
						stream.handle.write_all(&[2, 0, 1, b'-']).is_ok(),
						WriteHandshakePacketSnafu
					);
					stream.handle.flush().unwrap();
					tracing::debug!("username: {username:?}");
				}
				packets::CHAT_MESSAGE => {
					let message = stream.read_string()?;
					tracing::debug!("chat: {message}")
				}
				packets::PLAYER => {
					let on_ground = stream.read_u8()? == 1;

					tracing::debug!("on_ground: {on_ground}");
				}
				_ => {
					tracing::debug!("unknown packet: {:#04x}", packet_id);
				}
			}
		}
	}
}

fn main() -> Result<()> {
	tracing_subscriber::fmt::init();

	let listener = TcpListener::bind("[::]:25565");
	ensure!(listener.is_ok(), ListenFailedSnafu);
	let listener = listener.unwrap();

	let players = Arc::new(Mutex::new(vec![]));

	loop {
		let players = players.clone();

		match listener.accept() {
			Err(err) => {
				tracing::warn!("{}", err);
			}
			Ok(mut stream) => {
				std::thread::spawn(move || {
					handle_client(
						&mut PacketSerializer::new(&mut stream.0),
						&mut players.lock().unwrap(),
					)
					.unwrap();
				});
			}
		}
	}
}
