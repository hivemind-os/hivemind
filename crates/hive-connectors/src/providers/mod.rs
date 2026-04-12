pub mod coinbase;
pub mod discord;
pub mod gmail;
pub mod imap;
pub mod microsoft;
pub mod slack;

#[cfg(target_os = "macos")]
pub mod apple;
