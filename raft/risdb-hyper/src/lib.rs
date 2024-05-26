// pub mod client;
mod server;
mod helper;
mod client;

mod items {
    include!(concat!(env!("OUT_DIR"), "/risdb.proto.rs"));
}

pub use self::helper::get_workspace_base_dir;
pub use self::server::run;
pub use self::client::*;