//! This crate facilitates easy interprocess communication through the SysV IPC protocol
//! and Serde. Beware that there is an arbitrary message size limit set at 8KB.
//!
//! Through some `PhantomData` and Higher-Ranked Trait Bounds magic, MessageQueue is pretty
//! smart when it comes to type (de)serialization. Intentionally, MessageQueues are limited
//! to one data type, which is defined when you create a new queue.

// TODO implement a queue iterator
// TODO semaphores, shared memory
#![feature(associated_type_defaults)]
#![deny(missing_docs)]
extern crate serde_cbor;
extern crate serde;
extern crate libc;
extern crate nix;

use serde::{Deserialize, Serialize};
use nix::errno::errno;
use nix::errno::Errno;

use libc::{
	msgget,
	msgctl,
	msqid_ds,
};

use std::ptr;
//use std::mem;
use std::convert::From;
use std::marker::PhantomData;
use std::borrow::{Borrow, BorrowMut};

pub mod raw;
use raw::*;

/// An enum containing all possible IPC errors
#[derive(Debug, PartialEq, Eq)]
pub enum IpcError {
	/// Returned, when it wasn't possible to
	/// deserialize bytes into the desired types.
	/// That mostly occurs if MessageQueue is defined
	/// with the wrong type or the input got truncated
	FailedToDeserialize,
	/// Returned when an attempt was made to use
	/// a queue before the `init()` call was made
	QueueIsUninitialized,
	/// Returned, if it was impossible to read the `Message`
	/// structure. Occurs if you made the raw pointer too
	/// early and the underlying data got dropped already
	CouldntReadMessage,
	/// When the queue already exists, but IpcFlags::Exclusive
	/// and `IpcFlags::CreateKey` were both specified
	QueueAlreadyExists,
	/// Occurs if it isn't possible to serialize a struct
	/// into bytes. Shouldn't normally occur, might indicate
	/// a bug in the CBOR library
	FailedToSerialize,
	/// `IpcFlags::Exclusive` was specified, but queue
	/// doesn't exist
	QueueDoesntExist,
	/// The Queue has been removed (might be because the
	/// system ran out of memory)
	QueueWasRemoved,
	/// A signal was received
	SignalReceived,
	/// The message is invalid, occurs if the message struct
	/// does not follow the mtype-mtext forma
	InvalidMessage,
	/// Returned when an invalid command was given
	/// to `msgctl()`
	InvalidCommand,
	/// The message is bigger than either the system limit
	/// or the set limit
	MessageTooBig,
	/// Invalid struct
	///
	/// This error is returned when
	/// `msgctl()` was called with a invalid
	/// pointer to a struct, which would be
	/// either `msqid_ds or msginfo`.test
	InvalidStruct,
	/// There are too many `MessageQueue`s already
	/// (shouldn't occur, the limit is pretty big)
	TooManyQueues,
	/// Access was denied, you are trying to read a queue
	/// that doesn't belong to you or your process
	AccessDenied,
	/// The queue is full, 'nuff said
	QueueFull,
	/// There is no message. This isn't an error,
	/// per se, but the intended return value of
	/// nonblocking `recv()` calls
	NoMessage,
	/// There is not enough space left in the queue.
	/// This isn't really an error either, it is what
	/// is returned by a nonblocking `send()` call
	NoMemory,

	/// We know it was an error, but it was
	/// something non-standard
	UnknownErrorValue(i32),
	/// one of the standard functions returned
	/// a value it should never return.
	/// (for example `msgsnd()` returning 5)
	UnknownReturnValue(i32),
}

/// A helper enum for describing
/// a message queue access mode
///
/// Note that the creator of a message queue
/// bypasses permission mode and what's
/// described here applies to the owner
/// of the message queue (owner != creator).
#[derive(Debug, Clone, Copy)]
pub enum Mode {
	/// Allows complete access to anyone
	Public,
	/// Allows complete access to only
	/// the owner's group and the owner
	/// (and the creator)
	Group,
	/// Allows complete access to only
	/// the owner (and the creator)
	Private,
	/// Custom modes. Please, do try
	/// to make sure that you only
	/// pass numbers >= 0777
	Custom(i32),
}

impl From<Mode> for i32 {
	/// Allows conversion of modes to
	/// and from `i32`. This conversion
	/// can never fail, but there is a
	/// chance that numbers 'longer' than
	/// 9 bits might interfere with flags.
	///
	/// Therefore, only use custom mode
	/// when absolutely necessary.
	fn from(mode: Mode) -> i32 {
		match mode {
			Mode::Public => 0o666,
			Mode::Group => 0o660,
			Mode::Private => 0o600,
			Mode::Custom(x) => x,
		}
	}
}

/// The main message queue type.
/// It holds basic information about a given message queue
/// as well as type data about the content that passes
/// through it.
///
/// The `PhantomData` marker ensures that the queue
/// is locked to (de)serializing a single tyoe.
///
/// MessageQueue is quite liberal about the types
/// it accepts. If you are only ever going to send
/// a type, it just requires that the type is
/// `Serialize`.
///
/// If the queue is only ever going to be receiving
/// data, it requires the associated type to be
/// `Deserialize`.
///
/// This allows you to spare some precious bytes.
///
/// Note that `MessageQueue` reports all errors
/// properly, so they should be handled, lest you
/// wish to shoot your leg off.
///
/// ## General Usage Example
///
/// Before usage a `MessageQueue` needs to be initialized
/// through the use of the [`MessageQueue::init()`]
/// method. Failure to do so results in the queue
/// refusing to work.
///
/// ```no_run
/// # extern crate ipc_rs;
/// # use ipc_rs::IpcError;
/// use ipc_rs::MessageQueue;
///
/// # fn main() -> Result<(), IpcError> {
/// let my_key = 1234;
/// let queue = MessageQueue::<String>::new(my_key)
/// 	.create()
/// 	.async()
/// 	.init()?;
///
/// queue.send("hello world".to_string(), 24)
/// 	.expect("failed to send a message");
/// # Ok(())
/// # }
/// ```
pub struct MessageQueue<T> {
	/// The actual ID of the underlying SysV message
	/// queue. This value is 'unassigned' until a call
	/// to the `init()` method is made.
	pub id: i32,
	/// This is the key that was given when the
	/// `MessageQueue` was created, make sure to use
	/// the same key on both sides of the barricade
	/// to ensure proper 'connection' is estabilised
	pub key: i32,
	/// The bit flags used to create a new queue,
	/// see [`IpcFlags`] for more info.
	pub mask: i32,
	/// The bit flags used when sending/receiving a
	/// message, they for example affect whether data
	/// gets truncated or whether the calls to `send()`
	/// and `recv()` are blocking or not.
	pub message_mask: i32,
	/// Mode bits, these are an equivalent to those
	/// one encounters when working with files in Unix
	/// systems, therefore, the 9 least significant bits
	/// follow this pattern:
	///
	/// ```text
	/// rwxrwxrwx
	/// |_||_||_|
	///  │  │  │
	///  │  │  └── others
	///  │  └── owner's user group
	///  └── owner
	/// ```
	///
	/// Currently, the execute bits are ignored, so you
	/// needn't worry about them. Therefore, to allow
	/// full access to anyone, mode should be set to
	/// `0666` aka `0b110_110_110`.
	///
	/// Similarly, to make the queue `private` one would
	/// use `0600` aka `0b110_000_000`. `
	pub mode: i32,
	auto_kill: bool,
	initialized: bool,
	types: PhantomData<T>,
}

/// This struct represents a message that is inserted into
/// a message queue on every [`MessageQueue::send()`] call.
///
/// It follows the SysV message recipe of having only two
/// fields, namely:
///
/// * `mtype: i64` - which is the type of a message, it can
///   be used for filtering within queues and should never
///   be a negative integer. u64 isn't used here, however,
///   because of the kernel's anticipated internal representation
/// * `mtext` - which is where the data of the message are stored.
///   The kernel doesn't care about what `mtext` is so long
///   as it is not a pointer (because pointers are a recipe
///   for trouble when passing the interprocess boundary).
///   Therefore it can be either a struct or an array. Here,
///   an array of 8K bytes was chosen to allow the maximum
///   versatility within the default message size limit (8KiB).
///   In the future, functionality to affect the limit shall
///   be exposed and bigger messages will be allowed
///
/// Messages are required to be #[repr(C)] to avoid unexpected
/// surprises.
///
/// Finally, due to the size of a Message, it is unwise to
/// store them on the stack. On Arch x86_64, the default stack
/// size is 8mb, which is just enough for less than a thousand
/// messages. Use Box instead.
#[repr(C)]
pub struct Message {
	/// This should be a positive integer.
	/// For normal usage, it is inconsequential,
	/// but you may want to use it for filtering.
	///
	/// In fact, if you are looking for messages
	/// with a specific type, the `msgtyp` parameter
	/// of [`msgrcv()`] might be of use to you.
	///
	/// Check out its documentation for more info.
	pub mtype: i64,
	/// This is a simple byte array. The 'standard'
	/// allows for mtext to be either a structure
	/// or an array. For the purposes of `ipc-rs`,
	/// array is the better choice.
	///
	/// Currently, the data is stored as CBOR, the
	/// more efficient byte JSON. Check out the
	/// documentation of `serde_cbor`.
	pub mtext: [u8; 65536],
}

impl<T> Drop for MessageQueue<T> {
	/// Does nothing unless auto_kill is specified,
	/// in which case it deletes the associated queue
	fn drop(&mut self) {
		if self.auto_kill {
			let _ = self.delete(); // We don't really care about failures here
		}
	}
}

impl<T> MessageQueue<T> {
	/// Allow the creation of a new message queue
	pub fn create(mut self) -> Self {
		self.mask |= IpcFlags::CreateKey as i32;
		self
	}

	/// Enforce the operation at hand. If `create()`
	/// is also used, `init()` will fail if the create
	/// already exist.
	pub fn exclusive(mut self) -> Self {
		self.mask |= IpcFlags::Exclusive as i32;
		self
	}

	/// Adds the NoWait flag to message_mask to make
	/// the calls to `send()` and `recv()` non-blocking.
	/// When there is no message to be received, `recv()`
	/// returns [`IpcError::NoMessage`] and similarly,
	/// when a message can't be sent because the queue is
	/// full, a nonblocking `send()` returns [`IpcError::QueueFull`]
	pub fn async(mut self) -> Self {
		self.message_mask |= IpcFlags::NoWait as i32;
		self
	}

	/// Sets the mode of a given message queue.
	/// See [`Mode`] for more information
	pub fn mode(mut self, mode: Mode) -> Self {
		self.mode = mode.into();
		self
	}

	/// Automatically deletes removes a queue when it
	/// goes out of scope. That basically boils down
	/// to `self.delete()` being called during Drop
	pub fn auto_kill(mut self, kill: bool) -> Self {
		self.auto_kill = kill;
		self
	}

	/// Deletes a queue through `msgctl()`
	pub fn delete(&mut self) -> Result<(), IpcError> {
		if !self.initialized {
			return Err(IpcError::QueueIsUninitialized);
		}

		let res = unsafe {
			msgctl(self.id, ControlCommands::DeleteQueue as i32, ptr::null::<msqid_ds>() as *mut msqid_ds)
		};

		match res {
			-1 => match Errno::from_i32(errno()) {
			    Errno::EPERM  => Err(IpcError::AccessDenied),
			    Errno::EACCES => Err(IpcError::AccessDenied),
			    Errno::EFAULT => Err(IpcError::InvalidStruct),
			    Errno::EINVAL => Err(IpcError::InvalidCommand),
			    Errno::EIDRM  => Err(IpcError::QueueDoesntExist),
			    _ => Err(IpcError::UnknownErrorValue(errno())),
			}
			_ => { self.initialized = false; Ok(()) }
		}
	}

	/// Initializes a MessageQueue with the key
	/// `self.key`, proper modes and mask
	pub fn init(mut self) -> Result<Self, IpcError> {
		self.initialized = true;
		self.id = unsafe { msgget(self.key, self.mask | self.mode) };

		match self.id {
			-1 => match Errno::from_i32(errno()) {
				Errno::EEXIST => Err(IpcError::QueueAlreadyExists),
				Errno::ENOENT => Err(IpcError::QueueDoesntExist),
				Errno::ENOSPC => Err(IpcError::TooManyQueues),
				Errno::EACCES => Err(IpcError::AccessDenied),
				Errno::ENOMEM => Err(IpcError::NoMemory),
				_ => Err(IpcError::UnknownErrorValue(errno())),
			}
			_  => Ok(self),
		}
	}

	/// Defines a new `MessageQueue`
	/// In the future, it will be possible to use more types
	/// of keys (which would be translated to i32 behind the
	/// scenes automatically)
	pub fn new(key: i32) -> Self {
		MessageQueue {
			id: -1,
			key,
			mask: 0,
			message_mask: 0,
			mode: 0o666,
			initialized: false,
			auto_kill: false,
			types: PhantomData
		}
	}
}

impl<'a, T> MessageQueue<T> where T: Serialize {
	/// Sends a new message, or tries to (in case of non-blocking calls).
	/// If the queue is full, `IpcError::QueueFull` is returned
	pub fn send<I>(&self, src: T, mtype: I) -> Result<(), IpcError> where I: Into<i64> {
		if !self.initialized {
			return Err(IpcError::QueueIsUninitialized);
		}

		let mut message = Box::new(Message {
			mtype: mtype.into(),
			mtext: [0; 65536],
		});
		let bytes = match serde_cbor::ser::to_vec(&src) {
			Ok(b) => b,
			Err(_) => return Err(IpcError::FailedToSerialize),
		};

		bytes
			.iter()
			.enumerate()
			.for_each(|(i, x)| message.mtext[i] = *x);

		let res = unsafe {
			msgsnd(self.id, message.borrow() as *const Message, bytes.len(), 0)
		};

		match res {
			-1 => match Errno::from_i32(errno()) {
				Errno::EFAULT => Err(IpcError::CouldntReadMessage),
				Errno::EIDRM  => Err(IpcError::QueueWasRemoved),
				Errno::EINTR  => Err(IpcError::SignalReceived),
				Errno::EINVAL => Err(IpcError::InvalidMessage),
				Errno::E2BIG  => Err(IpcError::MessageTooBig),
				Errno::EACCES => Err(IpcError::AccessDenied),
				Errno::ENOMSG => Err(IpcError::NoMessage),
				Errno::EAGAIN => Err(IpcError::QueueFull),
				Errno::ENOMEM => Err(IpcError::NoMemory),
				_ => Err(IpcError::UnknownErrorValue(errno())),
			}
			0 => Ok(()),
			x => Err(IpcError::UnknownReturnValue(x as i32)),
		}
	}
}

impl<'a, T> MessageQueue<T> where for<'de> T: Deserialize<'de> {
	/// Returns a message without removing it from the message
	/// queue. Use `recv()` if you want to consume the message
	pub fn peek(&self) -> Result<T, IpcError> {
		if !self.initialized {
			return Err(IpcError::QueueIsUninitialized);
		}

		let mut message: Box<Message> = Box::new(Message {
			mtype: 0,
			mtext: [0; 65536],
		});

		let size = unsafe { msgrcv(self.id, message.borrow_mut() as *mut Message, 65536, 0, IpcFlags::MsgCopy as i32 | self.message_mask) };

		if size >= 0 {
			match serde_cbor::from_slice(&message.mtext[..size as usize]) {
				Ok(r) => Ok(r),
				Err(_) => Err(IpcError::FailedToDeserialize),
			}
		}
		else {
			match Errno::from_i32(errno()) {
				Errno::EFAULT => Err(IpcError::CouldntReadMessage),
				Errno::EIDRM  => Err(IpcError::QueueWasRemoved),
				Errno::EINTR  => Err(IpcError::SignalReceived),
				Errno::EINVAL => Err(IpcError::InvalidMessage),
				Errno::E2BIG  => Err(IpcError::MessageTooBig),
				Errno::EACCES => Err(IpcError::AccessDenied),
				Errno::ENOMSG => Err(IpcError::NoMessage),
				Errno::EAGAIN => Err(IpcError::QueueFull),
				Errno::ENOMEM => Err(IpcError::NoMemory),
				_ => Err(IpcError::UnknownErrorValue(errno())),
			}
		}
	}

	/// Receives a message, consuming it. If no message is
	/// to be received, `recv()` either blocks or returns
	/// [`IpcError::NoMemory`]
	pub fn recv(&self) -> Result<T, IpcError> {
		if !self.initialized {
			return Err(IpcError::QueueIsUninitialized);
		}

		let mut message: Box<Message> = Box::new(Message {
			mtype: 0,
			mtext: [0; 65536],
		}); // spooky scary stuff

		let size = unsafe { msgrcv(self.id, message.borrow_mut() as *mut Message, 65536, 0, self.message_mask) };

		if size >= 0 {
			match serde_cbor::from_slice(&message.mtext[..size as usize]) {
				Ok(r) => Ok(r),
				Err(_) => Err(IpcError::FailedToDeserialize),
			}
		}
		else {
			match Errno::from_i32(errno()) {
				Errno::EFAULT => Err(IpcError::CouldntReadMessage),
				Errno::EIDRM  => Err(IpcError::QueueWasRemoved),
				Errno::EINTR  => Err(IpcError::SignalReceived),
				Errno::EINVAL => Err(IpcError::InvalidMessage),
				Errno::E2BIG  => Err(IpcError::MessageTooBig),
				Errno::EACCES => Err(IpcError::AccessDenied),
				Errno::ENOMSG => Err(IpcError::NoMessage),
				Errno::EAGAIN => Err(IpcError::QueueFull),
				Errno::ENOMEM => Err(IpcError::NoMemory),
				_ => Err(IpcError::UnknownErrorValue(errno())),
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use ::MessageQueue;
	use ::IpcError;

	#[test]
	fn send_message() {
		let queue = MessageQueue::new(1234).init().unwrap();
		let res = queue.send("kalinka", 25);
		println!("{:?}", res);
		assert!(res.is_ok());
	}

	#[test]
	fn recv_message() {
		let queue = MessageQueue::<String>::new(1234).init().unwrap();
		let res = queue.recv();
		println!("{:?}", res);
		assert!(res.is_ok());
	}

	#[test]
	fn nonblocking() {
		let queue = MessageQueue::<()>::new(745965545)
			.async()
			.init()
			.unwrap();

		println!("{}", queue.mask);
		assert_eq!(Err(IpcError::NoMessage), queue.recv())
	}
}
