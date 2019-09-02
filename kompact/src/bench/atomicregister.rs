use super::*;

use crate::partitioning_actor::*;
use benchmark_suite_shared::kompics_benchmarks::benchmarks::AtomicRegisterRequest;
use kompact::prelude::*;
use kompact::*;
use partitioning_actor::PartitioningActor;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use synchronoise::CountdownEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct ClientParams {
    read_workload: f32,
    write_workload: f32,
}
impl ClientParams {
    fn new(read_workload: f32, write_workload: f32) -> ClientParams {
        ClientParams {
            read_workload,
            write_workload,
        }
    }
}

pub mod actor_atomicregister {
    use super::*;

    #[derive(Default)]
    pub struct AtomicRegister;

    impl DistributedBenchmark for AtomicRegister {
        type MasterConf = AtomicRegisterRequest;
        type ClientConf = ClientParams;
        type ClientData = ActorPath;
        type Master = AtomicRegisterMaster;
        type Client = AtomicRegisterClient;

        const LABEL: &'static str = "AtomicRegister";

        fn new_master() -> Self::Master {
            AtomicRegisterMaster::new()
        }

        fn msg_to_master_conf(
            msg: Box<dyn (::protobuf::Message)>,
        ) -> Result<Self::MasterConf, BenchmarkError> {
            downcast_msg!(msg; AtomicRegisterRequest)
        }

        fn new_client() -> Self::Client {
            AtomicRegisterClient::new()
        }

        fn str_to_client_conf(str: String) -> Result<Self::ClientConf, BenchmarkError> {
            let split: Vec<_> = str.split(',').collect();
            if split.len() != 2 {
                Err(BenchmarkError::InvalidMessage(format!(
                    "String '{}' does not represent a client conf!",
                    str
                )))
            } else {
                let readwl_str = split[0];
                let read_workload = readwl_str.parse::<f32>().map_err(|e| {
                    BenchmarkError::InvalidMessage(format!(
                        "String '{}' does not represent a client conf: {:?}",
                        str, e
                    ))
                })?;
                let writewl_str = split[1];
                let write_workload = writewl_str.parse::<f32>().map_err(|e| {
                    BenchmarkError::InvalidMessage(format!(
                        "String '{}' does not represent a client conf: {:?}",
                        str, e
                    ))
                })?;
                Ok(ClientParams::new(read_workload, write_workload))
            }
        }

        fn str_to_client_data(str: String) -> Result<Self::ClientData, BenchmarkError> {
            let res = ActorPath::from_str(&str);
            res.map_err(|e| {
                BenchmarkError::InvalidMessage(format!("Could not read client data: {}", e))
            })
        }

        fn client_conf_to_str(c: Self::ClientConf) -> String {
            format!("{},{}", c.read_workload, c.write_workload)
        }

        fn client_data_to_str(d: Self::ClientData) -> String {
            d.to_string()
        }
    }

    pub struct AtomicRegisterMaster {
        read_workload: Option<f32>,
        write_workload: Option<f32>,
        partition_size: Option<u32>,
        num_keys: Option<u64>,
        system: Option<KompactSystem>,
        finished_latch: Option<Arc<CountdownEvent>>,
        init_id: u32,
        atomic_register: Option<Arc<Component<AtomicRegisterActor>>>,
        partitioning_actor: Option<Arc<Component<PartitioningActor>>>,
    }

    impl AtomicRegisterMaster {
        fn new() -> AtomicRegisterMaster {
            AtomicRegisterMaster {
                read_workload: None,
                write_workload: None,
                partition_size: None,
                num_keys: None,
                system: None,
                finished_latch: None,
                init_id: 0,
                atomic_register: None,
                partitioning_actor: None,
            }
        }
    }

    impl DistributedBenchmarkMaster for AtomicRegisterMaster {
        type MasterConf = AtomicRegisterRequest;
        type ClientConf = ClientParams;
        type ClientData = ActorPath;

        fn setup(
            &mut self,
            c: Self::MasterConf,
            _m: &DeploymentMetaData,
        ) -> Result<Self::ClientConf, BenchmarkError> {
            println!("Setting up Atomic Register(Master)");
            self.read_workload = Some(c.read_workload);
            self.write_workload = Some(c.write_workload);
            self.partition_size = Some(c.partition_size);
            self.num_keys = Some(c.number_of_keys);
            let system =
                crate::kompact_system_provider::global().new_remote_system("atomicregister", 1);
            self.system = Some(system);
            let params = ClientParams {
                read_workload: c.read_workload,
                write_workload: c.write_workload,
            };
            Ok(params)
        }

        fn prepare_iteration(&mut self, d: Vec<Self::ClientData>) -> () {
            match self.system {
                Some(ref system) => {
                    println!("Preparing iteration");
                    let prepare_latch = Arc::new(CountdownEvent::new(1));
                    let finished_latch = Arc::new(CountdownEvent::new(1));
                    /*** Setup atomic register ***/
                    let (atomic_register, unique_reg_f) = system.create_and_register(|| {
                        AtomicRegisterActor::with(
                            self.read_workload.unwrap(),
                            self.write_workload.unwrap(),
                        )
                    });
                    let named_reg_f = system.register_by_alias(
                        &atomic_register,
                        format!("atomicreg_actor{}", &self.init_id),
                    );

                    unique_reg_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("AtomicRegisterComp never registered!")
                        .expect("AtomicRegisterComp to register!");

                    named_reg_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("AtomicRegisterComp never registered!")
                        .expect("AtomicRegisterComp to register!");

                    let atomic_register_f = system.start_notify(&atomic_register);
                    atomic_register_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("AtomicRegisterComp never started!");

                    /*** Add self path to vector of nodes ***/
                    let self_path = ActorPath::Named(NamedPath::with_system(
                        system.system_path(),
                        vec![format!("atomicreg_actor{}", &self.init_id).into()],
                    ));
                    let mut nodes: Vec<ActorPath> = Vec::new();
                    nodes.push(self_path);
                    for i in 0..(self.partition_size.unwrap() - 1) as usize {
                        nodes.push(d[i].clone());
                    }
                    /*** Setup partitioning actor ***/
                    let (partitioning_actor, unique_reg_f) = system.create_and_register(|| {
                        PartitioningActor::with(
                            prepare_latch.clone(),
                            finished_latch.clone(),
                            self.init_id,
                            nodes,
                            self.num_keys.unwrap(),
                        )
                    });
                    unique_reg_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("PartitioningComp never registered!")
                        .expect("PartitioningComp to register!");

                    let partitioning_actor_f = system.start_notify(&partitioning_actor);
                    partitioning_actor_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("PartitioningComp never started!");

                    self.init_id += 1;
                    self.finished_latch = Some(finished_latch);
                    self.atomic_register = Some(atomic_register);
                    self.partitioning_actor = Some(partitioning_actor);
                    prepare_latch.wait();
                    //                println!("Preparation successful!");
                }
                None => unimplemented!(),
            }
        }

        fn run_iteration(&mut self) -> () {
            match self.system {
                Some(ref system) => {
                    println!("Running experiment!");
                    let finished_latch = self.finished_latch.take().unwrap();
                    if let Some(partitioning_actor) = self.partitioning_actor.take() {
                        let partitioning_actor_ref = partitioning_actor.actor_ref();
                        partitioning_actor_ref.tell(Box::new(Run), system);
                        finished_latch.wait();
                        self.partitioning_actor = Some(partitioning_actor);
                    } else {
                        unimplemented!()
                    }
                }
                None => unimplemented!(),
            }
        }

        fn cleanup_iteration(&mut self, last_iteration: bool, _exec_time_millis: f64) -> () {
            println!("Cleaning up Atomic Register(master) side");
            let system = self.system.take().unwrap();
            let atomic_register = self.atomic_register.take().unwrap();
            let kill_atomic_reg_f = system.kill_notify(atomic_register);

            kill_atomic_reg_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Atomic Register Actor never died!");

            let partitioning_actor = self.partitioning_actor.take().unwrap();
            let kill_pactor_f = system.kill_notify(partitioning_actor);

            kill_pactor_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Partitioning Actor never died!");

            if last_iteration {
                println!("Cleaning up last iteration");
                system
                    .shutdown()
                    .expect("Kompact didn't shut down properly");

                self.read_workload = None;
                self.write_workload = None;
                self.num_keys = None;
                self.partition_size = None;
            } else {
                self.system = Some(system);
            }
        }
    }

    pub struct AtomicRegisterClient {
        system: Option<KompactSystem>,
        atomic_register: Option<Arc<Component<AtomicRegisterActor>>>,
    }

    impl AtomicRegisterClient {
        fn new() -> AtomicRegisterClient {
            AtomicRegisterClient {
                system: None,
                atomic_register: None,
            }
        }
    }

    impl DistributedBenchmarkClient for AtomicRegisterClient {
        type ClientConf = ClientParams;
        type ClientData = ActorPath;

        fn setup(&mut self, c: Self::ClientConf) -> Self::ClientData {
            println!("Setting up Atomic Register(client)");
            let system =
                crate::kompact_system_provider::global().new_remote_system("atomicregister", 1);
            let (atomic_register, unique_reg_f) = system.create_and_register(|| {
                AtomicRegisterActor::with(c.read_workload, c.write_workload)
            });
            let named_reg_f = system.register_by_alias(&atomic_register, "atomicreg_actor");
            unique_reg_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Atomic Register actor never registered!")
                .expect("Atomic Register actor failed to register!");
            named_reg_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Atomic Register actor never registered!")
                .expect("Atomic Register actor failed to register!");
            let start_f = system.start_notify(&atomic_register);
            start_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Atomic Register actor never started!");

            let named_path = ActorPath::Named(NamedPath::with_system(
                system.system_path(),
                vec!["atomicreg_actor".into()],
            ));
            self.atomic_register = Some(atomic_register);
            self.system = Some(system);
            println!("Got path for Atomic Register actor: {}", named_path);
            named_path
        }

        fn prepare_iteration(&mut self) -> () {
            println!("Preparing Atomic Register(client)");
        }

        fn cleanup_iteration(&mut self, last_iteration: bool) -> () {
            println!("Cleaning up Atomic Register(client) side");
            if last_iteration {
                let system = self.system.take().unwrap();
                let atomic_register = self.atomic_register.take().unwrap();
                let stop_f = system.kill_notify(atomic_register);
                stop_f
                    .wait_timeout(Duration::from_millis(1000))
                    .expect("Atomic Register actor never died!");

                system
                    .shutdown()
                    .expect("Kompact didn't shut down properly");
            }
        }
    }

    #[derive(ComponentDefinition)]
    struct AtomicRegisterActor {
        ctx: ComponentContext<AtomicRegisterActor>,
        read_workload: f32,
        write_workload: f32,

        master: Option<ActorPath>,
        nodes: Option<Vec<ActorPath>>,
        n: u32,
        rank: u32,
        min_key: u64,
        max_key: u64,
        read_count: u64,
        write_count: u64,
        current_run_id: u32,
        register_state: HashMap<u64, AtomicRegisterState>,
        register_readlist: HashMap<u64, HashMap<u32, (u32, u32, u32)>>,
    }

    impl AtomicRegisterActor {
        fn with(read_workload: f32, write_workload: f32) -> AtomicRegisterActor {
            AtomicRegisterActor {
                ctx: ComponentContext::new(),
                read_workload,
                write_workload,
                master: None,
                nodes: None,
                n: 0,
                rank: 0,
                min_key: 0,
                max_key: 0,
                read_count: 0,
                write_count: 0,
                current_run_id: 0,
                register_state: HashMap::<u64, AtomicRegisterState>::new(),
                register_readlist: HashMap::<u64, HashMap<u32, (u32, u32, u32)>>::new(),
            }
        }

        fn new_iteration(&mut self, init: &Init) -> () {
            self.current_run_id = init.init_id;
            let n_usize = init.nodes.len();
            self.n = n_usize as u32;
            self.rank = init.rank;
            self.min_key = init.min_key;
            self.max_key = init.max_key;
            let num_keys = ((&self.max_key - &self.min_key) + 1) as usize;
            self.register_state = HashMap::with_capacity(num_keys); // clears maps
            self.register_readlist = HashMap::with_capacity(num_keys);
            for i in init.min_key..=init.max_key {
                self.register_state.insert(i, AtomicRegisterState::new());
                self.register_readlist.insert(i, HashMap::new());
            }
        }

        fn invoke_read(&mut self, key: u64) -> () {
            let register = self.register_state.get_mut(&key).unwrap();
            register.rid += 1;
            register.acks = 0;
            register.reading = true;
            self.register_readlist.get_mut(&key).unwrap().clear();
            let read = Read {
                run_id: self.current_run_id,
                rid: register.rid,
                key,
            };
            self.bcast(AtomicRegisterMessage::Read(read));
        }

        fn invoke_write(&mut self, key: u64) -> () {
            let register = self.register_state.get_mut(&key).unwrap();
            register.rid += 1;
            register.writeval = self.rank;
            register.acks = 0;
            register.reading = false;
            self.register_readlist.get_mut(&key).unwrap().clear();
            let read = Read {
                run_id: self.current_run_id,
                rid: register.rid,
                key,
            };
            self.bcast(AtomicRegisterMessage::Read(read));
        }

        fn invoke_operations(&mut self) -> () {
            let num_keys = self.max_key - self.min_key + 1;
            let num_reads = (num_keys as f32 * self.read_workload) as u64;
            let num_writes = (num_keys as f32 * self.write_workload) as u64;
            self.read_count = num_reads;
            self.write_count = num_writes;
            if self.rank % 2 == 0 {
                for key in 0..num_reads {
                    self.invoke_read(key);
                }
                for l in 0..num_writes {
                    let key = self.min_key + num_reads + l;
                    self.invoke_write(key);
                }
            } else {
                for key in 0..num_writes {
                    self.invoke_write(key);
                }
                for l in 0..num_reads {
                    let key = self.min_key + num_writes + l;
                    self.invoke_read(key);
                }
            }
        }

        fn read_response(&mut self, _key: u64, _read_value: u32) -> () {
            self.read_count -= 1;
            if self.read_count == 0 && self.write_count == 0 {
                self.master
                    .as_ref()
                    .unwrap()
                    .tell((Done, PartitioningActorSer), self);
            }
        }

        fn write_response(&mut self, _key: u64) -> () {
            self.write_count -= 1;
            if self.read_count == 0 && self.write_count == 0 {
                self.master
                    .as_ref()
                    .unwrap()
                    .tell((Done, PartitioningActorSer), self);
            }
        }

        fn bcast(&self, msg: AtomicRegisterMessage) -> () {
            let nodes = self.nodes.as_ref().unwrap();
            for node in nodes {
                node.tell((msg.clone(), AtomicRegisterSer), self);
            }
        }
    }

    impl Provide<ControlPort> for AtomicRegisterActor {
        fn handle(&mut self, _event: ControlEvent) -> () {
            // ignore
        }
    }

    impl Actor for AtomicRegisterActor {
        fn receive_local(&mut self, _sender: ActorRef, _msg: &dyn Any) -> () {
            // ignore
        }

        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut dyn Buf) -> () {
            if ser_id == Serialiser::<Init>::serid(&PARTITIONING_ACTOR_SER) {
                let r: Result<Init, SerError> = PartitioningActorSer::deserialise(buf);
                match r {
                    Ok(init) => {
                        self.new_iteration(&init);
                        self.nodes = Some(init.nodes);
                        let init_ack = InitAck(self.current_run_id);
                        sender.tell((init_ack, PARTITIONING_ACTOR_SER), self);
                        self.master = Some(sender);
                    }
                    Err(e) => error!(self.ctx.log(), "Error deserialising Init: {:?}", e),
                }
            } else if ser_id == Serialiser::<Run>::serid(&PARTITIONING_ACTOR_SER) {
                let r: Result<Run, SerError> = PartitioningActorSer::deserialise(buf);
                match r {
                    Ok(_) => {
                        self.invoke_operations();
                    }
                    Err(e) => error!(self.ctx.log(), "Error deserialising Run: {:?}", e),
                }
            } else if ser_id == Serialiser::<AtomicRegisterMessage>::serid(&ATOMIC_REGISTER_SER) {
                let r: Result<AtomicRegisterMessage, SerError> =
                    AtomicRegisterSer::deserialise(buf);
                match r {
                    Ok(AtomicRegisterMessage::Read(read)) => {
                        if read.run_id == self.current_run_id {
                            let current_register = self.register_state.get(&read.key).unwrap();
                            let value = Value {
                                run_id: self.current_run_id,
                                key: read.key,
                                rid: read.rid,
                                ts: current_register.ts,
                                wr: current_register.wr,
                                value: current_register.value,
                                sender_rank: self.rank,
                            };
                            sender.tell(
                                (AtomicRegisterMessage::Value(value), AtomicRegisterSer),
                                self,
                            );
                        }
                    }
                    Ok(AtomicRegisterMessage::Value(v)) => {
                        if v.run_id == self.current_run_id {
                            let current_register = self.register_state.get_mut(&v.key).unwrap();
                            if v.rid == current_register.rid {
                                let readlist = self.register_readlist.get_mut(&v.key).unwrap();
                                if current_register.reading {
                                    if readlist.is_empty() {
                                        current_register.first_received_ts = v.ts;
                                        current_register.readval = v.value;
                                    } else if current_register.skip_impose {
                                        if current_register.first_received_ts != v.ts {
                                            current_register.skip_impose = false;
                                        }
                                    }
                                }
                                readlist.insert(v.sender_rank, (v.ts, v.wr, v.value));
                                if readlist.len() > (self.n / 2) as usize {
                                    if current_register.reading && current_register.skip_impose {
                                        current_register.value = current_register.readval;
                                        readlist.clear();
                                        let r = current_register.readval;
                                        self.read_response(v.key, r);
                                    } else {
                                        let (maxts, rr, readvalue) =
                                            readlist.values().max_by(|x, y| x.cmp(&y)).unwrap();
                                        current_register.readval = readvalue.to_owned();
                                        let write = if current_register.reading {
                                            Write {
                                                ts: maxts.to_owned(),
                                                wr: rr.to_owned(),
                                                value: readvalue.to_owned(),
                                                run_id: v.run_id,
                                                key: v.key,
                                                rid: v.rid,
                                            }
                                        } else {
                                            Write {
                                                ts: maxts.to_owned() + 1,
                                                wr: self.rank,
                                                value: current_register.writeval,
                                                run_id: v.run_id,
                                                key: v.key,
                                                rid: v.rid,
                                            }
                                        };
                                        readlist.clear();
                                        self.bcast(AtomicRegisterMessage::Write(write));
                                    }
                                }
                            }
                        }
                    }
                    Ok(AtomicRegisterMessage::Write(w)) => {
                        if w.run_id == self.current_run_id {
                            let current_register = self.register_state.get_mut(&w.key).unwrap();
                            if (w.ts, w.wr) > (current_register.ts, current_register.wr) {
                                current_register.ts = w.ts;
                                current_register.wr = w.wr;
                                current_register.value = w.value;
                            }
                        }
                        let ack = Ack {
                            run_id: w.run_id,
                            key: w.key,
                            rid: w.rid,
                        };
                        sender.tell((AtomicRegisterMessage::Ack(ack), AtomicRegisterSer), self);
                    }
                    Ok(AtomicRegisterMessage::Ack(a)) => {
                        if a.run_id == self.current_run_id {
                            let current_register = self.register_state.get_mut(&a.key).unwrap();
                            if a.rid == current_register.rid {
                                current_register.acks += 1;
                                if current_register.acks > self.n / 2 {
                                    current_register.acks = 0;
                                    if current_register.reading {
                                        let r = current_register.readval;
                                        self.read_response(a.key, r);
                                    } else {
                                        self.write_response(a.key);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => error!(
                        self.ctx.log(),
                        "Error deserialising AtomicRegisterMessage: {:?}", e
                    ),
                }
            }
        }
    }
}

pub mod mixed_atomicregister {
    use super::*;

    #[derive(Default)]
    pub struct AtomicRegister;

    impl DistributedBenchmark for AtomicRegister {
        type MasterConf = AtomicRegisterRequest;
        type ClientConf = ClientParams;
        type ClientData = ActorPath;
        type Master = AtomicRegisterMaster;
        type Client = AtomicRegisterClient;

        const LABEL: &'static str = "AtomicRegister";

        fn new_master() -> Self::Master {
            AtomicRegisterMaster::new()
        }

        fn msg_to_master_conf(
            msg: Box<dyn (::protobuf::Message)>,
        ) -> Result<Self::MasterConf, BenchmarkError> {
            downcast_msg!(msg; AtomicRegisterRequest)
        }

        fn new_client() -> Self::Client {
            AtomicRegisterClient::new()
        }

        fn str_to_client_conf(str: String) -> Result<Self::ClientConf, BenchmarkError> {
            let split: Vec<_> = str.split(',').collect();
            if split.len() != 2 {
                Err(BenchmarkError::InvalidMessage(format!(
                    "String '{}' does not represent a client conf!",
                    str
                )))
            } else {
                let readwl_str = split[0];
                let read_workload = readwl_str.parse::<f32>().map_err(|e| {
                    BenchmarkError::InvalidMessage(format!(
                        "String '{}' does not represent a client conf: {:?}",
                        str, e
                    ))
                })?;
                let writewl_str = split[1];
                let write_workload = writewl_str.parse::<f32>().map_err(|e| {
                    BenchmarkError::InvalidMessage(format!(
                        "String '{}' does not represent a client conf: {:?}",
                        str, e
                    ))
                })?;
                Ok(ClientParams::new(read_workload, write_workload))
            }
        }

        fn str_to_client_data(str: String) -> Result<Self::ClientData, BenchmarkError> {
            let res = ActorPath::from_str(&str);
            res.map_err(|e| {
                BenchmarkError::InvalidMessage(format!("Could not read client data: {}", e))
            })
        }

        fn client_conf_to_str(c: Self::ClientConf) -> String {
            format!("{},{}", c.read_workload, c.write_workload)
        }

        fn client_data_to_str(d: Self::ClientData) -> String {
            d.to_string()
        }
    }

    pub struct AtomicRegisterMaster {
        read_workload: Option<f32>,
        write_workload: Option<f32>,
        partition_size: Option<u32>,
        num_keys: Option<u64>,
        system: Option<KompactSystem>,
        finished_latch: Option<Arc<CountdownEvent>>,
        init_id: u32,
        atomic_register: Option<Arc<Component<AtomicRegisterComp>>>,
        partitioning_actor: Option<Arc<Component<PartitioningActor>>>,
        bcast_comp: Option<Arc<Component<BroadcastComp>>>,
    }

    impl AtomicRegisterMaster {
        fn new() -> AtomicRegisterMaster {
            AtomicRegisterMaster {
                read_workload: None,
                write_workload: None,
                partition_size: None,
                num_keys: None,
                system: None,
                finished_latch: None,
                init_id: 0,
                atomic_register: None,
                partitioning_actor: None,
                bcast_comp: None,
            }
        }
    }

    impl DistributedBenchmarkMaster for AtomicRegisterMaster {
        type MasterConf = AtomicRegisterRequest;
        type ClientConf = ClientParams;
        type ClientData = ActorPath;

        fn setup(
            &mut self,
            c: Self::MasterConf,
            _m: &DeploymentMetaData,
        ) -> Result<Self::ClientConf, BenchmarkError> {
            println!("Setting up Atomic Register(Master)");
            self.read_workload = Some(c.read_workload);
            self.write_workload = Some(c.write_workload);
            self.partition_size = Some(c.partition_size);
            self.num_keys = Some(c.number_of_keys);
            let system =
                crate::kompact_system_provider::global().new_remote_system("atomicregister", 1);
            self.system = Some(system);
            let params = ClientParams {
                read_workload: c.read_workload,
                write_workload: c.write_workload,
            };
            Ok(params)
        }

        fn prepare_iteration(&mut self, d: Vec<Self::ClientData>) -> () {
            match self.system {
                Some(ref system) => {
                    println!("Preparing iteration");
                    let prepare_latch = Arc::new(CountdownEvent::new(1));
                    let finished_latch = Arc::new(CountdownEvent::new(1));
                    /*** Setup Broadcast component ***/
                    let (bcast_comp, unique_reg_f) =
                        system.create_and_register(|| BroadcastComp::new());
                    let bcast_comp_f = system.start_notify(&bcast_comp);
                    bcast_comp_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("BroadcastComp never started!");

                    unique_reg_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("BroadcastComp never registered!")
                        .expect("BroadcastComp to register!");

                    /*** Setup atomic register ***/
                    let (atomic_register, unique_reg_f) = system.create_and_register(|| {
                        AtomicRegisterComp::with(
                            self.read_workload.unwrap(),
                            self.write_workload.unwrap(),
                            bcast_comp.actor_ref(),
                        )
                    });
                    let named_reg_f = system.register_by_alias(
                        &atomic_register,
                        format!("atomicreg_comp{}", &self.init_id),
                    );

                    unique_reg_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("AtomicRegisterComp never registered!")
                        .expect("AtomicRegisterComp to register!");

                    named_reg_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("AtomicRegisterComp never registered!")
                        .expect("AtomicRegisterComp to register!");

                    let atomic_register_f = system.start_notify(&atomic_register);
                    atomic_register_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("AtomicRegisterComp never started!");

                    /*** Add self path to vector of nodes ***/
                    let self_path = ActorPath::Named(NamedPath::with_system(
                        system.system_path(),
                        vec![format!("atomicreg_comp{}", &self.init_id).into()],
                    ));
                    let mut nodes: Vec<ActorPath> = Vec::new();
                    nodes.push(self_path.clone());
                    for i in 0..(self.partition_size.unwrap() - 1) as usize {
                        nodes.push(d[i].clone());
                    }
                    /*** Connect broadcast and atomic register ***/
                    on_dual_definition(
                        &bcast_comp,
                        &atomic_register,
                        |bcast_def, atomicreg_def| {
                            biconnect(&mut bcast_def.bcast_port, &mut atomicreg_def.bcast_port);
                        },
                    )
                    .expect("Could not connect components!");

                    /*** Setup partitioning actor ***/
                    let (partitioning_actor, unique_reg_f) = system.create_and_register(|| {
                        PartitioningActor::with(
                            prepare_latch.clone(),
                            finished_latch.clone(),
                            self.init_id,
                            nodes,
                            self.num_keys.unwrap(),
                        )
                    });
                    unique_reg_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("PartitioningComp never registered!")
                        .expect("PartitioningComp to register!");

                    let partitioning_actor_f = system.start_notify(&partitioning_actor);
                    partitioning_actor_f
                        .wait_timeout(Duration::from_millis(1000))
                        .expect("PartitioningComp never started!");

                    self.init_id += 1;
                    self.finished_latch = Some(finished_latch);
                    self.atomic_register = Some(atomic_register);
                    self.partitioning_actor = Some(partitioning_actor);
                    self.bcast_comp = Some(bcast_comp);
                    prepare_latch.wait();
                }
                None => unimplemented!(),
            }
        }

        fn run_iteration(&mut self) -> () {
            match self.system {
                Some(ref system) => {
                    println!("Running experiment!");
                    let finished_latch = self.finished_latch.take().unwrap();
                    if let Some(partitioning_actor) = self.partitioning_actor.take() {
                        let partitioning_actor_ref = partitioning_actor.actor_ref();
                        partitioning_actor_ref.tell(Box::new(Run), system);
                        finished_latch.wait();
                        self.partitioning_actor = Some(partitioning_actor);
                    } else {
                        unimplemented!()
                    }
                }
                None => unimplemented!(),
            }
        }

        fn cleanup_iteration(&mut self, last_iteration: bool, _exec_time_millis: f64) -> () {
            println!("Cleaning up Atomic Register(master) side");
            let system = self.system.take().unwrap();
            let atomic_register = self.atomic_register.take().unwrap();
            let kill_atomic_reg_f = system.kill_notify(atomic_register);

            kill_atomic_reg_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Atomic Register Actor never died!");

            let partitioning_actor = self.partitioning_actor.take().unwrap();
            let kill_pactor_f = system.kill_notify(partitioning_actor);

            kill_pactor_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Partitioning Actor never died!");

            let bcast_comp = self.bcast_comp.take().unwrap();
            let kill_bcast_f = system.kill_notify(bcast_comp);

            kill_bcast_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("BroadcastComponent never died!");

            if last_iteration {
                println!("Cleaning up last iteration");
                system
                    .shutdown()
                    .expect("Kompact didn't shut down properly");

                self.read_workload = None;
                self.write_workload = None;
                self.num_keys = None;
                self.partition_size = None;
            } else {
                self.system = Some(system);
            }
        }
    }

    pub struct AtomicRegisterClient {
        system: Option<KompactSystem>,
        atomic_register: Option<Arc<Component<AtomicRegisterComp>>>,
        bcast_comp: Option<Arc<Component<BroadcastComp>>>,
    }

    impl AtomicRegisterClient {
        fn new() -> AtomicRegisterClient {
            AtomicRegisterClient {
                system: None,
                atomic_register: None,
                bcast_comp: None,
            }
        }
    }

    impl DistributedBenchmarkClient for AtomicRegisterClient {
        type ClientConf = ClientParams;
        type ClientData = ActorPath;

        fn setup(&mut self, c: Self::ClientConf) -> Self::ClientData {
            println!("Setting up Atomic Register(client)");
            let system =
                crate::kompact_system_provider::global().new_remote_system("atomicregister", 1);
            /*** Setup Broadcast component ***/
            let (bcast_comp, unique_reg_f) = system.create_and_register(|| BroadcastComp::new());
            let bcast_comp_f = system.start_notify(&bcast_comp);
            bcast_comp_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("BroadcastComp never started!");
            unique_reg_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("BroadcastComp actor never registered!")
                .expect("BroadcastComp actor failed to register!");

            /*** Setup atomic register ***/
            let (atomic_register, unique_reg_f) = system.create_and_register(|| {
                AtomicRegisterComp::with(c.read_workload, c.write_workload, bcast_comp.actor_ref())
            });
            let named_reg_f = system.register_by_alias(&atomic_register, "atomicreg_comp");
            unique_reg_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Atomic Register actor never registered!")
                .expect("Atomic Register actor failed to register!");
            named_reg_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Atomic Register actor never registered!")
                .expect("Atomic Register actor failed to register!");
            let start_f = system.start_notify(&atomic_register);
            start_f
                .wait_timeout(Duration::from_millis(1000))
                .expect("Atomic Register actor never started!");

            let named_path = ActorPath::Named(NamedPath::with_system(
                system.system_path(),
                vec!["atomicreg_comp".into()],
            ));

            /*** Connect broadcast and atomic register **/
            on_dual_definition(&bcast_comp, &atomic_register, |bcast_def, atomicreg_def| {
                biconnect(&mut bcast_def.bcast_port, &mut atomicreg_def.bcast_port);
            })
            .expect("Could not connect components!");

            self.atomic_register = Some(atomic_register);
            self.bcast_comp = Some(bcast_comp);
            self.system = Some(system);
            println!("Got path for Atomic Register actor: {}", named_path);
            named_path
        }

        fn prepare_iteration(&mut self) -> () {
            println!("Preparing Atomic Register(client)");
        }

        fn cleanup_iteration(&mut self, last_iteration: bool) -> () {
            println!("Cleaning up Atomic Register(client) side");
            if last_iteration {
                let system = self.system.take().unwrap();
                let atomic_register = self.atomic_register.take().unwrap();
                let stop_f = system.kill_notify(atomic_register);
                stop_f
                    .wait_timeout(Duration::from_millis(1000))
                    .expect("Atomic Register actor never died!");

                let bcast_comp = self.bcast_comp.take().unwrap();
                let kill_bcast_f = system.kill_notify(bcast_comp);
                kill_bcast_f
                    .wait_timeout(Duration::from_millis(1000))
                    .expect("BroadcastComponent never died!");

                system
                    .shutdown()
                    .expect("Kompact didn't shut down properly");
            }
        }
    }

    struct RegisteredPath<'a> {
        actor_path: &'a ActorPath,
        ctx: &'a ComponentContext<BroadcastComp>,
    }

    impl<'a> ActorSource for RegisteredPath<'a> {
        fn path_resolvable(&self) -> PathResolvable {
            PathResolvable::Path(self.actor_path.clone())
        }
    }

    impl<'a> Dispatching for RegisteredPath<'a> {
        fn dispatcher_ref(&self) -> ActorRef {
            self.ctx.dispatcher_ref()
        }
    }

    struct CacheInfo {
        sender: ActorPath,
        nodes: Vec<ActorPath>,
    }
    struct CacheNodesAck;
    #[derive(Clone, Debug)]
    struct BroadcastRequest(AtomicRegisterMessage);
    struct BroadcastPort;

    impl Port for BroadcastPort {
        type Indication = ();
        type Request = BroadcastRequest;
    }

    #[derive(ComponentDefinition)]
    struct BroadcastComp {
        ctx: ComponentContext<BroadcastComp>,
        bcast_port: ProvidedPort<BroadcastPort, BroadcastComp>,
        nodes: Option<Vec<ActorPath>>,
        sender: Option<ActorPath>,
    }

    impl BroadcastComp {
        fn new() -> BroadcastComp {
            BroadcastComp {
                ctx: ComponentContext::new(),
                bcast_port: ProvidedPort::new(),
                nodes: None,
                sender: None,
            }
        }
    }

    impl Provide<ControlPort> for BroadcastComp {
        fn handle(&mut self, _event: <ControlPort as Port>::Request) -> () {
            // ignore
        }
    }

    impl Provide<BroadcastPort> for BroadcastComp {
        fn handle(&mut self, request: BroadcastRequest) -> () {
            let nodes = self.nodes.as_ref().unwrap();
            let sender = self.sender.as_ref().unwrap();
            let payload = request.0;
            let fake_path = RegisteredPath {
                actor_path: sender,
                ctx: &self.ctx,
            };
            for node in nodes {
                node.tell((payload.clone(), AtomicRegisterSer), &fake_path);
            }
        }
    }

    impl Actor for BroadcastComp {
        fn receive_local(&mut self, sender: ActorRef, msg: &dyn Any) -> () {
            if let Some(ref c) = msg.downcast_ref::<CacheInfo>() {
                self.nodes = Some(c.nodes.clone());
                self.sender = Some(c.sender.clone());
                sender.tell(Box::new(CacheNodesAck), self);
            } else {
                error!(self.ctx.log(), "Could not downcast to CacheNodes!");
            }
        }

        fn receive_message(&mut self, _sender: ActorPath, _ser_id: u64, _buf: &mut dyn Buf) -> () {
            // ignore
        }
    }

    #[derive(ComponentDefinition)]
    struct AtomicRegisterComp {
        ctx: ComponentContext<AtomicRegisterComp>,
        bcast_port: RequiredPort<BroadcastPort, AtomicRegisterComp>,
        bcast_ref: ActorRef,
        read_workload: f32,
        write_workload: f32,

        master: Option<ActorPath>,
        nodes: Option<Vec<ActorPath>>,
        n: u32,
        rank: u32,
        min_key: u64,
        max_key: u64,
        read_count: u64,
        write_count: u64,
        current_run_id: u32,
        register_state: HashMap<u64, AtomicRegisterState>,
        register_readlist: HashMap<u64, HashMap<u32, (u32, u32, u32)>>,
    }

    impl AtomicRegisterComp {
        fn with(
            read_workload: f32,
            write_workload: f32,
            bcast_ref: ActorRef,
        ) -> AtomicRegisterComp {
            AtomicRegisterComp {
                ctx: ComponentContext::new(),
                bcast_port: RequiredPort::new(),
                bcast_ref,
                read_workload,
                write_workload,
                master: None,
                nodes: None,
                n: 0,
                rank: 0,
                min_key: 0,
                max_key: 0,
                read_count: 0,
                write_count: 0,
                current_run_id: 0,
                register_state: HashMap::<u64, AtomicRegisterState>::new(),
                register_readlist: HashMap::<u64, HashMap<u32, (u32, u32, u32)>>::new(),
            }
        }

        fn new_iteration(&mut self, init: &Init) -> () {
            self.current_run_id = init.init_id;
            let n_usize = init.nodes.len();
            self.n = n_usize as u32;
            self.rank = init.rank;
            self.min_key = init.min_key;
            self.max_key = init.max_key;
            let num_keys = ((&self.max_key - &self.min_key) + 1) as usize;
            self.register_state = HashMap::with_capacity(num_keys); // clear maps
            self.register_readlist = HashMap::with_capacity(num_keys);
            for i in init.min_key..=init.max_key {
                self.register_state.insert(i, AtomicRegisterState::new());
                self.register_readlist.insert(i, HashMap::new());
            }
        }

        fn invoke_read(&mut self, key: u64) -> () {
            let register = self.register_state.get_mut(&key).unwrap();
            register.rid += 1;
            register.acks = 0;
            register.reading = true;
            self.register_readlist.get_mut(&key).unwrap().clear();
            let read = Read {
                run_id: self.current_run_id,
                rid: register.rid,
                key,
            };
            self.bcast_port
                .trigger(BroadcastRequest(AtomicRegisterMessage::Read(read)));
        }

        fn invoke_write(&mut self, key: u64) -> () {
            let register = self.register_state.get_mut(&key).unwrap();
            register.rid += 1;
            register.writeval = self.rank;
            register.acks = 0;
            register.reading = false;
            self.register_readlist.get_mut(&key).unwrap().clear();
            let read = Read {
                run_id: self.current_run_id,
                rid: register.rid,
                key,
            };
            self.bcast_port
                .trigger(BroadcastRequest(AtomicRegisterMessage::Read(read)));
        }

        fn invoke_operations(&mut self) -> () {
            let num_keys = self.max_key - self.min_key + 1;
            let num_reads = (num_keys as f32 * self.read_workload) as u64;
            let num_writes = (num_keys as f32 * self.write_workload) as u64;
            self.read_count = num_reads;
            self.write_count = num_writes;
            if self.rank % 2 == 0 {
                for key in 0..num_reads {
                    self.invoke_read(key);
                }
                for l in 0..num_writes {
                    let key = self.min_key + num_reads + l;
                    self.invoke_write(key);
                }
            } else {
                for key in 0..num_writes {
                    self.invoke_write(key);
                }
                for l in 0..num_reads {
                    let key = self.min_key + num_writes + l;
                    self.invoke_read(key);
                }
            }
        }

        fn read_response(&mut self, _key: u64, _read_value: u32) -> () {
            self.read_count -= 1;
            if self.read_count == 0 && self.write_count == 0 {
                self.master
                    .as_ref()
                    .unwrap()
                    .tell((Done, PartitioningActorSer), self);
            }
        }

        fn write_response(&mut self, _key: u64) -> () {
            self.write_count -= 1;
            if self.read_count == 0 && self.write_count == 0 {
                self.master
                    .as_ref()
                    .unwrap()
                    .tell((Done, PartitioningActorSer), self);
            }
        }
    }

    impl Provide<ControlPort> for AtomicRegisterComp {
        fn handle(&mut self, _event: <ControlPort as Port>::Request) -> () {
            // ignore
        }
    }

    impl Require<BroadcastPort> for AtomicRegisterComp {
        fn handle(&mut self, _event: <BroadcastPort as Port>::Indication) -> () {
            // ignore
        }
    }

    impl Actor for AtomicRegisterComp {
        fn receive_local(&mut self, _sender: ActorRef, msg: &dyn Any) -> () {
            if msg.is::<CacheNodesAck>() {
                let master = self.master.as_ref().unwrap();
                let init_ack = InitAck(self.current_run_id);
                master.tell((init_ack, PARTITIONING_ACTOR_SER), self);
            }
        }

        fn receive_message(&mut self, sender: ActorPath, ser_id: u64, buf: &mut dyn Buf) -> () {
            if ser_id == Serialiser::<Init>::serid(&PARTITIONING_ACTOR_SER) {
                let r: Result<Init, SerError> = PartitioningActorSer::deserialise(buf);
                match r {
                    Ok(init) => {
                        self.new_iteration(&init);
                        self.nodes = Some(init.nodes.clone());
                        self.master = Some(sender);
                        let self_path =
                            ActorPath::from((self.ctx.system().system_path(), self.ctx.id()));
                        &self.bcast_ref.tell(
                            Box::new(CacheInfo {
                                sender: self_path,
                                nodes: init.nodes,
                            }),
                            self,
                        );
                    }
                    Err(e) => error!(self.ctx.log(), "Error deserialising Init: {:?}", e),
                }
            } else if ser_id == Serialiser::<Run>::serid(&PARTITIONING_ACTOR_SER) {
                let r: Result<Run, SerError> = PartitioningActorSer::deserialise(buf);
                match r {
                    Ok(_) => {
                        self.invoke_operations();
                    }
                    Err(e) => error!(self.ctx.log(), "Error deserialising Run: {:?}", e),
                }
            } else if ser_id == Serialiser::<AtomicRegisterMessage>::serid(&ATOMIC_REGISTER_SER) {
                let r: Result<AtomicRegisterMessage, SerError> =
                    AtomicRegisterSer::deserialise(buf);
                match r {
                    Ok(AtomicRegisterMessage::Read(read)) => {
                        if read.run_id == self.current_run_id {
                            let current_register = self.register_state.get(&read.key).unwrap();
                            let value = Value {
                                run_id: self.current_run_id,
                                key: read.key,
                                rid: read.rid,
                                ts: current_register.ts,
                                wr: current_register.wr,
                                value: current_register.value,
                                sender_rank: self.rank,
                            };
                            sender.tell(
                                (AtomicRegisterMessage::Value(value), AtomicRegisterSer),
                                self,
                            );
                        }
                    }
                    Ok(AtomicRegisterMessage::Value(v)) => {
                        if v.run_id == self.current_run_id {
                            let current_register = self.register_state.get_mut(&v.key).unwrap();
                            if v.rid == current_register.rid {
                                let readlist = self.register_readlist.get_mut(&v.key).unwrap();
                                if current_register.reading {
                                    if readlist.is_empty() {
                                        current_register.first_received_ts = v.ts;
                                        current_register.readval = v.value;
                                    } else if current_register.skip_impose {
                                        if current_register.first_received_ts != v.ts {
                                            current_register.skip_impose = false;
                                        }
                                    }
                                }
                                readlist.insert(v.sender_rank, (v.ts, v.wr, v.value));
                                if readlist.len() > (self.n / 2) as usize {
                                    if current_register.reading && current_register.skip_impose {
                                        current_register.value = current_register.readval;
                                        readlist.clear();
                                        let r = current_register.readval;
                                        self.read_response(v.key, r);
                                    } else {
                                        let (maxts, rr, readvalue) =
                                            readlist.values().max_by(|x, y| x.cmp(&y)).unwrap();
                                        current_register.readval = readvalue.to_owned();
                                        let write = if current_register.reading {
                                            Write {
                                                ts: maxts.to_owned(),
                                                wr: rr.to_owned(),
                                                value: readvalue.to_owned(),
                                                run_id: v.run_id,
                                                key: v.key,
                                                rid: v.rid,
                                            }
                                        } else {
                                            Write {
                                                ts: maxts.to_owned() + 1,
                                                wr: self.rank,
                                                value: current_register.writeval,
                                                run_id: v.run_id,
                                                key: v.key,
                                                rid: v.rid,
                                            }
                                        };
                                        readlist.clear();
                                        self.bcast_port.trigger(BroadcastRequest(
                                            AtomicRegisterMessage::Write(write),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    Ok(AtomicRegisterMessage::Write(w)) => {
                        if w.run_id == self.current_run_id {
                            let current_register = self.register_state.get_mut(&w.key).unwrap();
                            if (w.ts, w.wr) > (current_register.ts, current_register.wr) {
                                current_register.ts = w.ts;
                                current_register.wr = w.wr;
                                current_register.value = w.value;
                            }
                        }
                        let ack = Ack {
                            run_id: w.run_id,
                            key: w.key,
                            rid: w.rid,
                        };
                        sender.tell((AtomicRegisterMessage::Ack(ack), AtomicRegisterSer), self);
                    }
                    Ok(AtomicRegisterMessage::Ack(a)) => {
                        if a.run_id == self.current_run_id {
                            let current_register = self.register_state.get_mut(&a.key).unwrap();
                            if a.rid == current_register.rid {
                                current_register.acks += 1;
                                if current_register.acks > self.n / 2 {
                                    current_register.acks = 0;
                                    if current_register.reading {
                                        let r = current_register.readval;
                                        self.read_response(a.key, r);
                                    } else {
                                        self.write_response(a.key);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => error!(
                        self.ctx.log(),
                        "Error deserialising AtomicRegisterMessage: {:?}", e
                    ),
                }
            }
        }
    }
}

struct AtomicRegisterState {
    ts: u32,
    wr: u32,
    value: u32,
    acks: u32,
    readval: u32,
    writeval: u32,
    rid: u32,
    reading: bool,
    first_received_ts: u32,
    skip_impose: bool,
}

impl AtomicRegisterState {
    fn new() -> AtomicRegisterState {
        AtomicRegisterState {
            reading: false,
            skip_impose: true,
            ts: 0,
            wr: 0,
            value: 0,
            acks: 0,
            readval: 0,
            writeval: 0,
            rid: 0,
            first_received_ts: 0,
        }
    }
}

#[derive(Clone, Debug)]
struct Start;
#[derive(Clone, Debug)]
struct Read {
    run_id: u32,
    key: u64,
    rid: u32,
}
#[derive(Clone, Debug)]
struct Ack {
    run_id: u32,
    key: u64,
    rid: u32,
}
#[derive(Clone, Debug)]
struct Value {
    run_id: u32,
    key: u64,
    rid: u32,
    ts: u32,
    wr: u32,
    value: u32,
    sender_rank: u32, // use as key in readlist map
}
#[derive(Clone, Debug)]
struct Write {
    run_id: u32,
    key: u64,
    rid: u32,
    ts: u32,
    wr: u32,
    value: u32,
}

struct AtomicRegisterSer;
const ATOMIC_REGISTER_SER: AtomicRegisterSer = AtomicRegisterSer {};
const READ_ID: i8 = 1;
const WRITE_ID: i8 = 2;
const VALUE_ID: i8 = 3;
const ACK_ID: i8 = 4;

#[derive(Clone, Debug)]
enum AtomicRegisterMessage {
    Read(Read),
    Value(Value),
    Write(Write),
    Ack(Ack),
}

impl Serialiser<AtomicRegisterMessage> for AtomicRegisterSer {
    fn serid(&self) -> u64 {
        serialiser_ids::ATOMICREG_ID
    }

    fn size_hint(&self) -> Option<usize> {
        Some(33) // TODO: Set it dynamically? 33 is for the largest message(Value)
    }

    fn serialise(&self, enm: &AtomicRegisterMessage, buf: &mut dyn BufMut) -> Result<(), SerError> {
        match enm {
            AtomicRegisterMessage::Read(r) => {
                buf.put_i8(READ_ID);
                buf.put_u32_be(r.run_id);
                buf.put_u64_be(r.key);
                buf.put_u32_be(r.rid);
                Ok(())
            }
            AtomicRegisterMessage::Value(v) => {
                buf.put_i8(VALUE_ID);
                buf.put_u32_be(v.run_id);
                buf.put_u64_be(v.key);
                buf.put_u32_be(v.rid);
                buf.put_u32_be(v.ts);
                buf.put_u32_be(v.wr);
                buf.put_u32_be(v.value);
                buf.put_u32_be(v.sender_rank);
                Ok(())
            }
            AtomicRegisterMessage::Write(w) => {
                buf.put_i8(WRITE_ID);
                buf.put_u32_be(w.run_id);
                buf.put_u64_be(w.key);
                buf.put_u32_be(w.rid);
                buf.put_u32_be(w.ts);
                buf.put_u32_be(w.wr);
                buf.put_u32_be(w.value);
                Ok(())
            }
            AtomicRegisterMessage::Ack(a) => {
                buf.put_i8(ACK_ID);
                buf.put_u32_be(a.run_id);
                buf.put_u64_be(a.key);
                buf.put_u32_be(a.rid);
                Ok(())
            }
        }
    }
}

impl Deserialiser<AtomicRegisterMessage> for AtomicRegisterSer {
    fn deserialise(buf: &mut dyn Buf) -> Result<AtomicRegisterMessage, SerError> {
        match buf.get_i8() {
            READ_ID => {
                let run_id = buf.get_u32_be();
                let key = buf.get_u64_be();
                let rid = buf.get_u32_be();
                Ok(AtomicRegisterMessage::Read(Read { run_id, key, rid }))
            }
            VALUE_ID => {
                let run_id = buf.get_u32_be();
                let key = buf.get_u64_be();
                let rid = buf.get_u32_be();
                let ts = buf.get_u32_be();
                let wr = buf.get_u32_be();
                let value = buf.get_u32_be();
                let sender_rank = buf.get_u32_be();
                Ok(AtomicRegisterMessage::Value(Value {
                    run_id,
                    key,
                    rid,
                    ts,
                    wr,
                    value,
                    sender_rank,
                }))
            }
            WRITE_ID => {
                let run_id = buf.get_u32_be();
                let key = buf.get_u64_be();
                let rid = buf.get_u32_be();
                let ts = buf.get_u32_be();
                let wr = buf.get_u32_be();
                let value = buf.get_u32_be();
                Ok(AtomicRegisterMessage::Write(Write {
                    run_id,
                    key,
                    rid,
                    ts,
                    wr,
                    value,
                }))
            }
            ACK_ID => {
                let run_id = buf.get_u32_be();
                let key = buf.get_u64_be();
                let rid = buf.get_u32_be();
                Ok(AtomicRegisterMessage::Ack(Ack { run_id, key, rid }))
            }

            _ => Err(SerError::InvalidType(
                "Found unkown id, but expected Read, Value, Write or Ack.".into(),
            )),
        }
    }
}
