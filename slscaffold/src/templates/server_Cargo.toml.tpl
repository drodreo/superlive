[package]
name = "__ID__-server"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "__ID__-server"
path = "src/main.rs"

[dependencies]
# Plane B contract SDK: serve() skeleton (listening / token auth / health / SIGTERM)
# and the manifest types shared by both sides of the contract.
__SLSDK_RS_LINE__

axum = "0.8"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
serde_json = "1"
log = "0.4"
env_logger = "0.11"
anyhow = "1"
