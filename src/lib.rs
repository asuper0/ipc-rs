//! This crate facilitates easy interprocess communication through the SysV IPC protocol
//! and Serde. Beware that there is an arbitrary message size limit set at 8KB.
//!
//! Through some `PhantomData` and Higher-Ranked Trait Bounds magic, MessageQueue is pretty
//! smart when it comes to type (de)serialization. Intentionally, MessageQueues are limited
//! to one data type, which is defined when you create a new queue.

// TODO implement a queue iterator
// TODO semaphores, shared memory
// #![feature(associated_type_defaults)]
// #![deny(missing_docs)]
extern crate libc;
#[cfg(unix)]
extern crate nix;
extern crate serde;

#[cfg(unix)]
pub mod linux;
#[cfg(unix)]
pub use linux::*;

#[cfg(windows)]
pub mod win;
#[cfg(windows)]
pub use win::*;
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
