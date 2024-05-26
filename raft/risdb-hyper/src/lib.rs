// pub mod client;
mod client;
mod helper;
mod server;

mod items {
    include!(concat!(env!("OUT_DIR"), "/risdb.proto.rs"));
}

pub use self::client::*;
pub use self::helper::get_workspace_base_dir;
pub use self::server::run;
