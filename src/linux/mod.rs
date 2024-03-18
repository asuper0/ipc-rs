// use serde_cbor;
// use serde;
// use libc;
// use nix;

use nix::errno::errno;
use nix::errno::Errno;
use serde::{Deserialize, Serialize};

use libc::{msgctl, msgget, msqid_ds};

use std::ptr;
use std::marker::PhantomData;

pub mod raw;
use raw::*;

use crate::{IpcError, Mode};

const MSG_TYPE_FIELD_LEN: usize = 8;

impl From<Errno> for IpcError {
    fn from(value: Errno) -> Self {
        match value {
            Errno::EFAULT => IpcError::CouldntReadMessage,
            Errno::EIDRM => IpcError::QueueWasRemoved,
            Errno::EINTR => IpcError::SignalReceived,
            Errno::EINVAL => IpcError::InvalidMessage,
            Errno::E2BIG => IpcError::MessageTooBig,
            Errno::EPERM => IpcError::AccessDenied,
            Errno::EACCES => IpcError::AccessDenied,
            Errno::ENOMSG => IpcError::NoMessage,
            Errno::EAGAIN => IpcError::QueueFull,
            Errno::ENOMEM => IpcError::NoMemory,
            Errno::EEXIST => IpcError::QueueAlreadyExists,
            Errno::ENOENT => IpcError::QueueDoesntExist,
            Errno::ENOSPC => IpcError::TooManyQueues,
            _ => IpcError::UnknownErrorValue(errno()),
        }
    }
}

/// The main message queue type.
/// It holds basic information about a given message queue
/// as well as type data about the content that passes
/// through it.
///
/// The `PhantomData` marker ensures that the queue
/// is locked to (de)serializing a single type.
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
/// let queue = MessageQueue::<String>::new(my_key, 128)
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
    /// to ensure proper 'connection' is established
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
    max_size: usize,
    types: PhantomData<T>,
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
    pub fn to_async(mut self) -> Self {
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
            msgctl(
                self.id,
                ControlCommands::DeleteQueue as i32,
                ptr::null::<msqid_ds>() as *mut msqid_ds,
            )
        };

        match res {
            -1 => {
                let err = Errno::from_i32(errno());
                match err {
                    Errno::EFAULT => Err(IpcError::InvalidStruct),
                    Errno::EINVAL => Err(IpcError::InvalidCommand),
                    Errno::EIDRM => Err(IpcError::QueueDoesntExist),
                    _ => Err(err.into()),
                }
            }
            _ => {
                self.initialized = false;
                Ok(())
            }
        }
    }

    /// Initializes a MessageQueue with the key
    /// `self.key`, proper modes and mask
    pub fn init(mut self) -> Result<Self, IpcError> {
        self.initialized = true;
        self.id = unsafe { msgget(self.key, self.mask | self.mode) };

        match self.id {
            -1 => Err(Errno::from_i32(errno()).into()),
            _ => Ok(self),
        }
    }

    /// Defines a new `MessageQueue`
    /// In the future, it will be possible to use more types
    /// of keys (which would be translated to i32 behind the
    /// scenes automatically)
    pub fn new(key: i32, max_size: usize) -> Self {
        MessageQueue {
            id: -1,
            key,
            mask: 0,
            message_mask: 0,
            mode: 0o666,
            initialized: false,
            auto_kill: false,
            types: PhantomData,
            max_size: max_size + MSG_TYPE_FIELD_LEN,
        }
    }
}

impl<'a, T> MessageQueue<T>
where
    T: Serialize,
{
    /// Sends a new message, or tries to (in case of non-blocking calls).
    /// If the queue is full, `IpcError::QueueFull` is returned
    pub fn send<I>(&self, src: T, mtype: I) -> Result<(), IpcError>
    where
        I: Into<i64>,
    {
        if !self.initialized {
            return Err(IpcError::QueueIsUninitialized);
        }

        let mut buffer: Vec<u8> = Vec::with_capacity(self.max_size);
        let mtype: i64 = mtype.into();
        buffer.extend(mtype.to_le_bytes());
        if let Err(_) = serde_cbor::ser::to_writer(&mut buffer, &src) {
             return Err(IpcError::FailedToSerialize);
        }

        let res = unsafe {
            msgsnd(
                self.id,
                buffer.as_ptr(),
                buffer.len() - MSG_TYPE_FIELD_LEN,
                0,
            )
        };

        match res {
            -1 => Err(Errno::from_i32(errno()).into()),
            0 => Ok(()),
            x => Err(IpcError::UnknownReturnValue(x as i32)),
        }
    }
}

impl<'a, T> MessageQueue<T>
where
    for<'de> T: Deserialize<'de>,
{
    /// Returns a message without removing it from the message
    /// queue. Use `recv()` if you want to consume the message
    pub fn peek(&self) -> Result<T, IpcError> {
        self.inner_recv(IpcFlags::MsgCopy as i32 | self.message_mask)
    }

    /// Receives a message, consuming it. If no message is
    /// to be received, `recv()` either blocks or returns
    /// [`IpcError::NoMemory`]
    pub fn recv(&self) -> Result<T, IpcError> {
        self.inner_recv(self.message_mask)
    }

    fn inner_recv(&self, msg_flag: i32) -> Result<T, IpcError> {
        if !self.initialized {
            return Err(IpcError::QueueIsUninitialized);
        }

        let mut buffer = Vec::with_capacity(self.max_size);

        let size = unsafe {
            let size = msgrcv(
                self.id,
                buffer.as_mut_ptr(),
                self.max_size,
                0,
                msg_flag,
            );
            if size >= 0 {
                buffer.set_len(size as usize);
            }
            size
        };

        if size >= 0 {
            match serde_cbor::from_slice(
                &buffer[MSG_TYPE_FIELD_LEN..MSG_TYPE_FIELD_LEN + size as usize],
            ) {
                Ok(r) => Ok(r),
                Err(_) => Err(IpcError::FailedToDeserialize),
            }
        } else {
            Err(Errno::from_i32(errno()).into())
        }
    }
}

#[cfg(test)]
mod tests {
    use IpcError;
    use MessageQueue;

    #[test]
    fn send_message() {
        let queue = MessageQueue::new(1234, 128).init().unwrap();
        let res = queue.send("kalinka", 25);
        println!("{:?}", res);
        assert!(res.is_ok());
    }

    #[test]
    fn recv_message() {
        let queue = MessageQueue::<String>::new(1234, 128).init().unwrap();
        let res = queue.recv();
        println!("{:?}", res);
        assert!(res.is_ok());
    }

    #[test]
    fn nonblocking() {
        let queue = MessageQueue::<()>::new(745965545, 128)
            .to_async()
            .init()
            .unwrap();

        println!("{}", queue.mask);
        assert_eq!(Err(IpcError::NoMessage), queue.recv())
    }
}
