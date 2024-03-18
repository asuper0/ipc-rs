use crate::{IpcError, Mode};
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

/// Bit flags for `msgget`
#[repr(i32)]
pub enum IpcFlags {
    /// Create the queue if it doesn't exist
    CreateKey,
    /// Fail if not creating and queue doesn't
    /// exist and fail if creating and queue
    /// already exists
    Exclusive,
    /// Make `send()` and `recv()` async
    NoWait,
    /// Allow truncation of message data
    MsgNoError,
    /// Used with `msgtyp` greater than 0
    /// to read the first message in the
    /// queue with a different type than
    /// `msgtyp`
    MsgExcept,
    /// Copy the message instead of removing
    /// it from the queue. Used for `peek()`
    MsgCopy,
}
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

        self.initialized = false;
        Ok(())
    }

    /// Initializes a MessageQueue with the key
    /// `self.key`, proper modes and mask
    pub fn init(mut self) -> Result<Self, IpcError> {
        self.initialized = true;
        self.id = 1;

        Ok(self)
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
            types: PhantomData,
        }
    }
}

impl<'a, T> MessageQueue<T>
where
    T: Serialize,
{
    /// Sends a new message, or tries to (in case of non-blocking calls).
    /// If the queue is full, `IpcError::QueueFull` is returned
    #[allow(unused_variables)]
    pub fn send<I>(&self, src: T, mtype: I) -> Result<(), IpcError>
    where
        I: Into<i64>,
    {
        if !self.initialized {
            return Err(IpcError::QueueIsUninitialized);
        }

        Ok(())
    }
}

impl<'a, T> MessageQueue<T>
where
    for<'de> T: Deserialize<'de>,
{
    /// Returns a message without removing it from the message
    /// queue. Use `recv()` if you want to consume the message
    pub fn peek(&self) -> Result<T, IpcError> {
        if !self.initialized {
            return Err(IpcError::QueueIsUninitialized);
        }

        let message: Box<Message> = Box::new(Message {
            mtype: 0,
            mtext: [0; 65536],
        });
        let size = 0;
        match serde_cbor::from_slice(&message.mtext[..size as usize]) {
            Ok(r) => Ok(r),
            Err(_) => Err(IpcError::FailedToDeserialize),
        }
    }

    /// Receives a message, consuming it. If no message is
    /// to be received, `recv()` either blocks or returns
    /// [`IpcError::NoMemory`]
    pub fn recv(&self) -> Result<T, IpcError> {
        if !self.initialized {
            return Err(IpcError::QueueIsUninitialized);
        }

        let message: Box<Message> = Box::new(Message {
            mtype: 0,
            mtext: [0; 65536],
        }); // spooky scary stuff

        let size = 0;
        match serde_cbor::from_slice(&message.mtext[..size as usize]) {
            Ok(r) => Ok(r),
            Err(_) => Err(IpcError::FailedToDeserialize),
        }
    }
}
