use kompact::prelude::SerId;

/* serids for Partitioning Actor messages */
pub const PARTITIONING_ID: SerId = 45;

pub const ATOMICREG_ID: SerId = 46;

pub const PING_ID: SerId = 50;
pub const PONG_ID: SerId = 51;
pub const STATIC_PING_ID: SerId = 52;
pub const STATIC_PONG_ID: SerId = 53;

pub const SW_SOURCE_ID: SerId = 54;
pub const SW_SINK_ID: SerId = 55;
pub const SW_WINDOWER_ID: SerId = 56;

pub const RAFT_ID: SerId = 57;
pub const BLE_ID: SerId = 58;
pub const ATOMICBCAST_ID: SerId = 59;
pub const PAXOS_ID: SerId = 60;
pub const REPLICA_ID: SerId = 61;

