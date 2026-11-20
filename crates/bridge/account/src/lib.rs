#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod account;

pub use account::{
    AccountEventKind, AccountSnapshot, AccountSummary, AuthorizationLink, BridgeFailure,
    CredentialState, RemoteRevocation, SessionActivity, SessionFailure, SessionFailureKind,
    SignOutStage, SourceSnapshot, convert,
};
