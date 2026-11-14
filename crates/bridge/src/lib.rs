#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod connection;

pub use connection::{
    AccountEvent, AccountEventKind, AccountSnapshot, AccountSummary, AuthorizationLink,
    BridgeFailure, Connection, CredentialState, ModelFailure, ModelFailureKind, ModelReply,
    RemoteRevocation, SessionActivity, SessionFailure, SessionFailureKind, SignOutStage,
};
