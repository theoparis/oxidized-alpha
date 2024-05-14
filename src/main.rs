#![deny(warnings)]

use irox_bits::{Bits, MutBits};
use miniz_oxide::deflate::compress_to_vec_zlib;
use oxidized_alpha::{packets, Chunk, Player};
use snafu::{ensure, Snafu};
use std::{
	io::{Read, Write},
	net::{TcpListener, TcpStream},
	sync::{
		atomic::{AtomicI32, Ordering},
		Arc, Mutex,
	},
};

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

	pub fn write_chunk(
		&mut self,
		chunk: Chunk,
	) -> Result<(), irox_bits::Error> {
		self.write_u8(packets::PRE_CHUNK)?;
		self.write_be_i32(chunk.x)?;
		self.write_be_i32(chunk.z)?;
		self.write_u8(0x01)?;

		let x = chunk.x * 16;
		let y = 0i16;
		let z = chunk.z * 16;

		let mut to_compress = chunk.blocks.clone();
		to_compress.extend_from_slice(&chunk.data);
		to_compress.extend_from_slice(&chunk.block_light);
		to_compress.extend_from_slice(&chunk.sky_light);

		let compressed = compress_to_vec_zlib(&to_compress, 6);

		// Send map chunk packet
		self.write_u8(packets::MAP_CHUNK)?;
		self.write_be_i32(x)?;
		self.write_be_i16(y)?;
		self.write_be_i32(z)?;

		self.write_u8(15)?;
		self.write_u8(127)?;
		self.write_u8(15)?;

		self.write_be_i32(compressed.len() as i32)?;
		self.write_all_bytes(&compressed)?;

		Ok(())
	}
}

static ENTITY_COUNTER: AtomicI32 = AtomicI32::new(1);

fn handle_client(
	stream: &mut PacketSerializer<TcpStream>,
	players: &mut Vec<Player>,
) -> Result<()> {
	let mut username: Option<String> = None;

	loop {
		if let Ok(packet_id) = stream.read_u8() {
			match packet_id {
				packets::KEEP_ALIVE => {
					stream.write_u8(0)?;
				}
				packets::LOGIN => {
					// Read login request packet
					// TODO: use irox-bits
					let protocol_version = stream.read_be_u32()?;

					tracing::debug!("protocol version: {}", protocol_version);
					ensure!(protocol_version == 3, InvalidProtocolVersionSnafu);

					username = Some(stream.read_string()?);
					let _password = stream.read_string()?;

					let map_seed = stream.read_be_u64()?;

					tracing::debug!("map seed: {}", map_seed);

					let dimension = stream.read_u8()?;
					tracing::debug!("dimension: {}", dimension);

					let entity_id =
						ENTITY_COUNTER.fetch_add(1, Ordering::Relaxed);

					// Send login response packet
					stream.write_u8(packets::LOGIN)?;
					stream.write_be_i32(entity_id)?;
					// Two unused/empty strings
					stream.write_be_u16(0)?;
					stream.write_be_u16(0)?;
					stream.write_be_u64(map_seed)?;
					stream.write_u8(dimension)?;

					let player = Player {
						username: username.clone().unwrap(),
						logged_in: false,
						x: 0.0,
						y: 80.0,
						z: 0.0,
						yaw: 0.0,
						pitch: 0.0,
						stance: 81.6,
						on_ground: true,
					};
					players.push(player.clone());
					let player_index = players.len() - 1;
					let player = players.get_mut(player_index).unwrap();

					let mut initial_chunk = Chunk {
						x: 1,
						z: 1,
						..Default::default()
					};

					for _i in 0..16 {
						for _k in 0..16 {
							for _j in 0..128 {
								initial_chunk.blocks.push(0);
								initial_chunk.data.push(0);
								initial_chunk.block_light.push(15);
								initial_chunk.sky_light.push(15);
							}
						}
					}

					assert_eq!(initial_chunk.blocks.len(), 16 * 128 * 16);

					stream.write_chunk(initial_chunk)?;
					tracing::debug!("Wrote map data");

					// Write spawn position packet
					stream.write_u8(packets::SPAWN_POSITION)?;
					stream.write_be_i32(player.x as i32)?;
					stream.write_be_i32(player.y as i32)?;
					stream.write_be_i32(player.z as i32)?;

					tracing::debug!("Wrote spawn position");

					// Write position and look packet
					stream.write_u8(packets::PLAYER_POSITION_AND_LOOK)?;
					stream.write_f64(player.x)?;
					stream.write_f64(player.stance)?;
					stream.write_f64(player.y)?;
					stream.write_f64(player.z)?;
					stream.write_f32(player.yaw)?;
					stream.write_f32(player.pitch)?;
					stream.write_u8(player.on_ground as u8)?;

					tracing::debug!("Wrote player rotation and position");
					player.logged_in = true;
				}
				packets::PLAYER_POSITION_AND_LOOK => {
					if let Some(player) = players.iter_mut().find(|player| {
						player.username == username.clone().unwrap()
					}) {
						player.x = stream.read_f64()?;
						player.stance = stream.read_f64()?;
						player.y = stream.read_f64()?;
						player.z = stream.read_f64()?;
						player.yaw = stream.read_f32()?;
						player.pitch = stream.read_f32()?;
						player.on_ground = stream.read_u8()? == 1;
					}
				}
				packets::PLAYER_POSITION => {
					if let Some(player) = players.iter_mut().find(|player| {
						player.username == username.clone().unwrap()
					}) {
						player.x = stream.read_f64()?;
						player.y = stream.read_f64()?;
						player.stance = stream.read_f64()?;
						player.z = stream.read_f64()?;
						player.on_ground = stream.read_u8()? == 1;
					}
				}
				packets::PLAYER_LOOK => {
					if let Some(player) = players.iter_mut().find(|player| {
						player.username == username.clone().unwrap()
					}) {
						player.yaw = stream.read_f32()?;
						player.pitch = stream.read_f32()?;
						player.on_ground = stream.read_u8()? == 1;
					}
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

					if let Some(player) = players.iter_mut().find(|player| {
						player.username == username.clone().unwrap()
					}) {
						player.on_ground = on_ground;
					}
				}
				_ => {
					tracing::error!("unknown packet: {:#04x}", packet_id);
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
