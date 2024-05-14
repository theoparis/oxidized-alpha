use std::{
	io::{Read, Write},
	net::{TcpListener, TcpStream},
};

use mc::packets;
use snafu::{ensure, Snafu};

pub type Result<T, E = Error> = core::result::Result<T, E>;

#[derive(Debug, Snafu)]
pub enum Error {
	ReadString,
	ReadStringLength,
	InvalidString,
	WriteHandshakePacket,
	WriteKeepAlivePacket,
	ReadOnGroundPacket,
	ListenFailed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Player {
	pub username: String,
}

// TODO: use irox-bits traits
pub fn read_string<Reader: Read>(src: &mut Reader) -> Result<String> {
	let mut length_buffer = [0u8; 2];
	ensure!(
		src.read_exact(&mut length_buffer).is_ok(),
		ReadStringLengthSnafu
	);
	let length = u16::from_be_bytes(length_buffer);

	let mut buf = vec![0u8; length as usize];
	ensure!(src.read_exact(&mut buf).is_ok(), ReadStringSnafu);

	let result = String::from_utf8(buf);
	ensure!(result.is_ok(), InvalidStringSnafu);

	Ok(result.unwrap())
}

fn handle_client(stream: &mut TcpStream) -> Result<()> {
	loop {
		let mut packet_id = vec![0u8; 1];

		if stream.read_exact(&mut packet_id).is_ok() {
			match packet_id[0] {
				packets::KEEP_ALIVE => {
					let packet = vec![0];
					ensure!(
						stream.write_all(&packet).is_ok(),
						WriteKeepAlivePacketSnafu
					);
					stream.flush().unwrap();
				}
				packets::LOGIN => {
					// TODO: login
				}
				// Handshake
				packets::HANDSHAKE => {
					let username = read_string(stream)?;
					ensure!(
						stream.write_all(&[2, 0, 1, b'-']).is_ok(),
						WriteHandshakePacketSnafu
					);
					stream.flush().unwrap();
					tracing::debug!("username: {username:?}");
				}
				packets::CHAT_MESSAGE => {
					let message = read_string(stream)?;
					tracing::debug!("chat: {message}")
				}
				packets::PLAYER => {
					let mut buf = vec![0u8; 1];
					ensure!(
						stream.read_exact(&mut buf).is_ok(),
						ReadOnGroundPacketSnafu
					);
					let on_ground = buf[0] == 1;

					tracing::debug!("on_ground: {on_ground}");
				}
				_ => {
					tracing::debug!("unknown packet: {:#04x}", packet_id[0]);
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

	loop {
		match listener.accept() {
			Err(err) => {
				tracing::warn!("{}", err);
			}
			Ok(mut stream) => {
				std::thread::spawn(move || {
					handle_client(&mut stream.0).unwrap();
				});
			}
		}
	}
}
