use crate::{
    benchmark::*,
    benchmark_runner::{
        measure, not_implemented, rse, run_async, DistributedIteration, MAX_RUNS, MIN_RUNS,
        RSE_TARGET,
    },
    kompics_benchmarks::{
        benchmarks, benchmarks_grpc, distributed,
        distributed_grpc::{self, BenchmarkClient},
        messages,
    },
};
use crossbeam::channel as cbchannel;
use futures::{future, sync::oneshot, Future};
use grpc::ClientStubExt;
#[allow(unused_imports)]
use slog::{crit, debug, error, info, o, warn, Drain, Logger};
use std::{
    panic::UnwindSafe,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

pub fn run(
    runner_port: u16,
    master_port: u16,
    wait_for: usize,
    benchmarks: Box<BenchmarkFactory>,
    logger: Logger,
) -> ()
{
    let (check_in_sender, check_in_receiver) = cbchannel::unbounded();
    let (bench_sender, bench_receiver) = cbchannel::unbounded();
    let mut inst = BenchmarkMaster::new(
        logger.new(
            o!("runner-port" => runner_port, "wait-for" => wait_for, "ty" => "BenchmarkMaster"),
        ),
        wait_for,
        check_in_receiver,
        bench_receiver,
    );

    // MASTER HANDLER
    let master_handler = MasterHandler::new(
        logger.new(o!("master-port" => master_port, "ty" => "MasterHandler")),
        inst.state(),
        check_in_sender,
    );
    let master_address = format!("0.0.0.0:{}", master_port);
    let mut serverb = grpc::ServerBuilder::new_plain();
    serverb
        .http
        .set_addr(master_address.clone())
        .expect(&format!("Could not use address: {}.", master_address));
    serverb.add_service(distributed_grpc::BenchmarkMasterServer::new_service_def(master_handler));
    let master_server = serverb.build().expect("master server");
    info!(logger, "MasterServer running on {}", master_server.local_addr());

    // RUNNER HANDLER
    let runner_handler = RunnerHandler::new(
        logger.new(o!("runner-port" => runner_port, "ty" => "RunnerHandler")),
        benchmarks,
        bench_sender,
        inst.state(),
    );
    let runner_address = format!("0.0.0.0:{}", runner_port);
    let mut serverb = grpc::ServerBuilder::new_plain();
    serverb
        .http
        .set_addr(runner_address.clone())
        .expect(&format!("Could not use address: {}.", runner_address));
    serverb.add_service(benchmarks_grpc::BenchmarkRunnerServer::new_service_def(runner_handler));
    let runner_server = serverb.build().expect("runner server");
    info!(logger, "RunnerServer running on {}", runner_server.local_addr());

    inst.start();
}

#[derive(Clone)]
pub(crate) struct ClientEntry {
    address: String,
    port:    u16,
    stub:    Arc<distributed_grpc::BenchmarkClientClient>,
}

impl ClientEntry {
    pub(crate) fn new(
        address: String,
        port: u16,
        stub: distributed_grpc::BenchmarkClientClient,
    ) -> ClientEntry
    {
        ClientEntry { address, port, stub: Arc::new(stub) }
    }

    pub(crate) fn cleanup(
        &self,
        is_final: bool,
    ) -> impl Future<Item = distributed::CleanupResponse, Error = grpc::Error>
    {
        let mut msg = distributed::CleanupInfo::new();
        msg.set_field_final(is_final);
        self.stub.cleanup(::grpc::RequestOptions::default(), msg).drop_metadata()
    }

    pub(crate) fn setup(
        &self,
        msg: distributed::SetupConfig,
    ) -> impl Future<Item = distributed::SetupResponse, Error = grpc::Error>
    {
        self.stub.setup(::grpc::RequestOptions::default(), msg).drop_metadata()
    }

    pub(crate) fn shutdown(
        &self,
        msg: messages::ShutdownRequest,
    ) -> impl Future<Item = messages::ShutdownAck, Error = grpc::Error>
    {
        self.stub.shutdown(::grpc::RequestOptions::default(), msg).drop_metadata()
    }
}

//type BenchClosure = Box<FnBox() -> Future<Item = messages::TestResult, Error = grpc::Error> + Send>;

enum BenchRequest {
    Invoke { invocation: BenchInvocation, promise: oneshot::Sender<messages::TestResult> },
    Shutdown(bool),
}
impl BenchRequest {
    fn new(
        invocation: BenchInvocation,
        promise: oneshot::Sender<messages::TestResult>,
    ) -> BenchRequest
    {
        BenchRequest::Invoke { invocation, promise }
    }
}

struct BenchInvocation {
    benchmark: AbstractBench,
    msg:       Box<::protobuf::Message + UnwindSafe>,
}
impl BenchInvocation {
    fn new<M: ::protobuf::Message + UnwindSafe>(
        benchmark: AbstractBench,
        msg: M,
    ) -> BenchInvocation
    {
        BenchInvocation { benchmark, msg: Box::new(msg) }
    }

    fn new_local<M: ::protobuf::Message + UnwindSafe>(
        benchmark: Box<AbstractBenchmark>,
        msg: M,
    ) -> BenchInvocation
    {
        BenchInvocation { benchmark: AbstractBench::Local(benchmark), msg: Box::new(msg) }
    }

    fn new_distributed<M: ::protobuf::Message + UnwindSafe>(
        benchmark: Box<AbstractDistributedBenchmark>,
        msg: M,
    ) -> BenchInvocation
    {
        BenchInvocation {
            benchmark: AbstractBench::Distributed(benchmark),
            msg:       Box::new(msg),
        }
    }
}

struct BenchmarkMaster {
    logger:         Logger,
    wait_for:       usize,
    clients:        Vec<ClientEntry>,
    state:          StateHolder,
    check_in_queue: cbchannel::Receiver<distributed::ClientInfo>,
    bench_queue:    cbchannel::Receiver<BenchRequest>,
}

impl BenchmarkMaster {
    fn new(
        logger: Logger,
        wait_for: usize,
        check_in_queue: cbchannel::Receiver<distributed::ClientInfo>,
        bench_queue: cbchannel::Receiver<BenchRequest>,
    ) -> BenchmarkMaster
    {
        BenchmarkMaster {
            logger,
            wait_for,
            clients: Vec::new(),
            state: StateHolder::init(),
            check_in_queue,
            bench_queue,
        }
    }

    fn state(&self) -> StateHolder { self.state.clone() }

    fn start(&mut self) -> () {
        info!(self.logger, "Starting...");
        while self.state.get() == State::INIT {
            let ci = self.check_in_queue.recv().expect("Queue to MasterHandler broke!");
            self.check_in_handler(ci);
        }

        loop {
            match self.state.get() {
                State::READY => {
                    debug!(self.logger, "Awaiting benchmark request");
                    let bench = self.bench_queue.recv().expect("Queue to RunnerHandler broke!");
                    match bench {
                        BenchRequest::Invoke { promise, invocation } => {
                            self.bench_request_handler(promise, invocation)
                        },
                        BenchRequest::Shutdown(force) => {
                            let mut sreq = messages::ShutdownRequest::new();
                            sreq.set_force(force);
                            info!(
                                self.logger,
                                "Sending shutdown to {} children.",
                                self.clients.len()
                            );
                            let f_list = self.clients.drain(..).map(move |c| {
                                c.shutdown(sreq.clone()).map(|res| (res, c)) // prevent the client from being deallocated early
                            });
                            let shutdown_f = future::join_all(f_list);
                            info!(self.logger, "Waiting for children to shut down.");
                            shutdown_f.wait().map(|_| ()).unwrap_or_else(|e| {
                                warn!(
                                    self.logger,
                                    "Some children may not have shut down properly: {:?}", e
                                );
                            });
                            self.state.assign(State::STOPPED);
                        },
                    }
                },
                State::STOPPED => {
                    info!(self.logger, "Master stopped!");
                    thread::sleep(Duration::from_millis(500)); // give it a bit of grace time
                    return;
                },
                s => {
                    debug!(self.logger, "Master waiting in state={:?}", s);
                    thread::sleep(Duration::from_millis(500));
                },
            }
        }
    }

    // internal use only

    fn check_in_handler(&mut self, request: distributed::ClientInfo) -> () {
        if self.state.get() == State::INIT {
            info!(
                self.logger,
                "Got Check-In from {}:{}",
                request.get_address(),
                request.get_port(),
            );
            let ce = self.client_info_to_entry(request);
            self.clients.push(ce);
            if self.clients.len() == self.wait_for {
                info!(self.logger, "Got all {} Check-Ins: Ready!", self.clients.len());
                self.state.cas(State::INIT, State::READY).expect("Wrong state!");
            } else {
                debug!(self.logger, "Got {}/{} Check-Ins.", self.clients.len(), self.wait_for);
            }
        } else {
            warn!(self.logger, "Ignoring late Check-In: {:?}", request);
        }
    }

    fn client_info_to_entry(&self, mut ci: distributed::ClientInfo) -> ClientEntry {
        let port = ci.get_port() as u16;
        let stub = distributed_grpc::BenchmarkClientClient::new_plain(
            ci.get_address(),
            port,
            Default::default(),
        )
        .expect(&format!("Could not connect to client {:?}", ci));
        ClientEntry::new(ci.take_address(), port, stub)
    }

    fn bench_request_handler(
        &mut self,
        promise: oneshot::Sender<messages::TestResult>,
        invocation: BenchInvocation,
    ) -> ()
    {
        let msg = invocation.msg;
        let (res, label) = match invocation.benchmark {
            AbstractBench::Local(b) => {
                let label = b.label();
                let f = self.run_local_benchmark(b, msg);
                (f.wait(), label)
            },
            AbstractBench::Distributed(b) => {
                let label = b.label();
                let f = self.run_distributed_benchmark(b, msg);
                (f.wait(), label)
            },
        };
        let blogger = self.logger.new(o!("benchmark" => label));
        match res {
            Ok(tr) => {
                promise.send(tr).expect("Receiver was closed?!?");
            },
            Err(e) => {
                error!(blogger, "Benchmark Future failed horribly! {:?}", e);
                drop(promise); // this will cancel the future
            },
        }
    }

    fn run_local_benchmark(
        &mut self,
        b: Box<AbstractBenchmark>,
        msg: Box<::protobuf::Message + UnwindSafe>,
    ) -> impl Future<Item = messages::TestResult, Error = BenchmarkError>
    {
        self.state.cas(State::READY, State::RUN).expect("Wasn't ready to run!");
        let blogger = self.logger.new(o!("benchmark" => b.label()));
        info!(blogger, "Starting local test {}", b.label());
        let f = run_async(move || b.run(msg).into());
        let state_copy = self.state.clone();
        f.then(move |res| {
            info!(blogger, "Completed local test.");
            state_copy.assign(State::READY);
            res
        })
    }

    fn run_distributed_benchmark(
        &mut self,
        b: Box<AbstractDistributedBenchmark>,
        msg: Box<::protobuf::Message + UnwindSafe>,
    ) -> impl Future<Item = messages::TestResult, Error = BenchmarkError>
    {
        let blogger = self.logger.new(o!("benchmark" => b.label()));
        let state_copy = self.state.clone();
        let state_copy2 = self.state.clone();
        let clients_copy1 = self.clients.clone();
        //let clients_copy2 = self.clients.clone();
        let bench_label = b.label();
        self.state.cas(State::READY, State::SETUP).expect("Wasn't ready to setup!");
        info!(blogger, "Starting distributed test {}", bench_label);

        let master_f: future::FutureResult<Box<AbstractBenchmarkMaster>, BenchmarkError> =
            future::ok(b.new_master());
        let master_cconf_f = master_f.and_then(|mut master| {
            future::result(master.setup(msg)).map(|client_conf| (master, client_conf))
        });
        let data_logger = blogger.clone();
        let client_data_f = master_cconf_f.and_then(move |(master, client_conf)| {
            let mut client_setup = distributed::SetupConfig::new();
            client_setup.set_label(bench_label.into());
            client_setup.set_data(client_conf.into());
            let f_list = clients_copy1.into_iter().map(move |c| {
                c.setup(client_setup.clone()).map_err(|e| e.into()).and_then(|sr| {
                    let res = if sr.success {
                        let cdh: ClientDataHolder = sr.data.into();
                        Ok((c, cdh))
                    } else {
                        Err(BenchmarkError::InvalidTest(sr.data))
                    };
                    future::result(res)
                })
            });
            info!(data_logger, "Awaiting client data.");
            future::join_all(f_list).map(|client_data| (master, client_data))
        });     
        let iter_logger = blogger.clone();
        let result_f = client_data_f.and_then(move |(master, client_data_l)| {
            debug!(iter_logger, "Collected all client data.");
            state_copy.cas(State::SETUP, State::RUN).expect("Running without setup?!?");
            let blogger = iter_logger; // just lazy to rename all uses
            let iteration = DistributedIteration::new(master, client_data_l);
            future::loop_fn(iteration, move |mut it| {
                let n_runs = it.n_runs();
                debug!(blogger, "Preparing iteration {}", n_runs);
                it = it.prepare();
                debug!(blogger, "Starting iteration {}", n_runs);
                it = it.run();
                debug!(blogger, "Finished iteration {}", n_runs);
                state_copy
                    .cas(State::RUN, State::CLEANUP)
                    .expect("Wasn't running before cleanup!?!");
                let itf: impl Future<Item = (DistributedIteration, bool), Error = grpc::Error> =
                    it.cleanup();
                let state_copy2 = state_copy.clone();
                let itlf = itf.map(move |(it, is_final)| {
                    if is_final {
                        state_copy2
                            .cas(State::CLEANUP, State::FINISHED)
                            .expect("Wasn't cleanup before run!");
                        let tr: messages::TestResult = Ok(it.results()).into();
                        future::Loop::Break(tr)
                    } else {
                        state_copy2
                            .cas(State::CLEANUP, State::RUN)
                            .expect("Wasn't cleanup before run!");
                        future::Loop::Continue(it)
                    }
                });
                itlf.map_err(|e| e.into())
            })
        });
        result_f
            .then(move |res: Result<messages::TestResult, BenchmarkError>| {
                info!(blogger, "Completed distributed test.");
                state_copy2.assign(State::READY);
                res
            })
            .map_err(|e| e.into())
    }
}

struct MasterHandler {
    logger:         Logger,
    state:          StateHolder,
    check_in_queue: cbchannel::Sender<distributed::ClientInfo>,
}

impl MasterHandler {
    fn new(
        logger: Logger,
        state: StateHolder,
        check_in_queue: cbchannel::Sender<distributed::ClientInfo>,
    ) -> MasterHandler
    {
        MasterHandler { logger, state, check_in_queue }
    }
}

impl distributed_grpc::BenchmarkMaster for MasterHandler {
    fn check_in(
        &self,
        _o: ::grpc::RequestOptions,
        p: distributed::ClientInfo,
    ) -> ::grpc::SingleResponse<distributed::CheckinResponse>
    {
        if self.state.get() == State::INIT {
            info!(self.logger, "Got Check-In from {}:{}", p.get_address(), p.get_port(),);
            self.check_in_queue.send(p).unwrap();
        } else {
            warn!(self.logger, "Ignoring late Check-In: {:?}", p);
        }
        grpc::SingleResponse::completed(distributed::CheckinResponse::new())
    }
}

struct RunnerHandler {
    logger:      Logger,
    benchmarks:  Box<BenchmarkFactory>,
    bench_queue: cbchannel::Sender<BenchRequest>,
    state:       StateHolder,
}

impl RunnerHandler {
    fn new(
        logger: Logger,
        benchmarks: Box<BenchmarkFactory>,
        bench_queue: cbchannel::Sender<BenchRequest>,
        state: StateHolder,
    ) -> RunnerHandler
    {
        RunnerHandler { logger, benchmarks, bench_queue, state }
    }

    fn enqeue(
        &self,
        inv: BenchInvocation,
    ) -> impl Future<Item = messages::TestResult, Error = grpc::Error>
    {
        let (promise, future) = oneshot::channel::<messages::TestResult>();
        let req = BenchRequest::new(inv, promise);
        match self.bench_queue.send(req) {
            Ok(_) => (), // yay
            Err(e) => {
                error!(self.logger, "Error sending BenchRequest: {:?}", e);
                drop(e); // this will cancel the future
            },
        }
        future.map_err(|e| grpc::Error::Canceled(e))
    }

    fn enqueue_if_implemented<B, F>(
        &self,
        res: Result<B, NotImplementedError>,
        f: F,
    ) -> grpc::SingleResponse<messages::TestResult>
    where
        F: FnOnce(B) -> BenchInvocation,
    {
        match res {
            Ok(b) => {
                let br = f(b);
                let f = self.enqeue(br);
                grpc::SingleResponse::no_metadata(f)
            },
            Err(e) => {
                warn!(self.logger, "Test finished with error: {:?}", e);
                let mut msg = messages::TestResult::new();
                msg.set_not_implemented(messages::NotImplemented::new());
                grpc::SingleResponse::completed(msg)
            },
        }
    }
}

impl benchmarks_grpc::BenchmarkRunner for RunnerHandler {
    fn ready(
        &self,
        _o: grpc::RequestOptions,
        _p: messages::ReadyRequest,
    ) -> grpc::SingleResponse<messages::ReadyResponse>
    {
        info!(self.logger, "Got ready? req.");
        let mut msg = messages::ReadyResponse::new();
        if self.state.get() == State::READY {
            msg.set_status(true);
        } else {
            msg.set_status(false);
        }
        grpc::SingleResponse::completed(msg)
    }

    fn shutdown(
        &self,
        _o: ::grpc::RequestOptions,
        p: messages::ShutdownRequest,
    ) -> ::grpc::SingleResponse<messages::ShutdownAck>
    {
        info!(self.logger, "Got shutdown request: {:?}", p);
        self.bench_queue.send(BenchRequest::Shutdown(p.force)).expect("Command channel broke!"); // make sure children get shut down
        if p.force {
            crate::force_shutdown();
        }
        grpc::SingleResponse::completed(messages::ShutdownAck::new())
    }

    fn ping_pong(
        &self,
        _o: grpc::RequestOptions,
        p: benchmarks::PingPongRequest,
    ) -> grpc::SingleResponse<messages::TestResult>
    {
        info!(self.logger, "Got ping-pong req: {}", p.get_number_of_messages());
        let b_res = self.benchmarks.pingpong();
        self.enqueue_if_implemented(b_res, |b| BenchInvocation::new(b.into(), p))
    }

    fn net_ping_pong(
        &self,
        _o: grpc::RequestOptions,
        p: benchmarks::PingPongRequest,
    ) -> grpc::SingleResponse<messages::TestResult>
    {
        info!(self.logger, "Got net-ping-pong req: {}", p.get_number_of_messages());
        let b_res = self.benchmarks.netpingpong();
        self.enqueue_if_implemented(b_res, |b| BenchInvocation::new(b.into(), p))
    }
}

#[derive(Clone)]
struct StateHolder(Arc<Mutex<State>>);

impl StateHolder {
    fn init() -> StateHolder { StateHolder(Arc::new(Mutex::new(State::INIT))) }

    fn assign(&self, v: State) -> () {
        let mut state = self.0.lock().unwrap();
        *state = v;
    }

    fn cas(&self, old_value: State, new_value: State) -> Result<(), StateError> {
        let mut state = self.0.lock().unwrap();
        if *state == old_value {
            *state = new_value;
            Ok(())
        } else {
            Err(StateError::InvalidTransition {
                from:     *state,
                to:       new_value,
                expected: old_value,
            })
        }
    }

    fn get(&self) -> State {
        let state = self.0.lock().unwrap();
        *state
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum State {
    INIT,
    READY,
    SETUP,
    RUN,
    CLEANUP,
    FINISHED,
    STOPPED,
}

#[derive(Debug, Clone)]
enum StateError {
    InvalidTransition { from: State, to: State, expected: State },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::tests::TestFactory;

    // #[test]
    // fn logging() {
    //     run(5, 5, 5000, TestFactory::boxed());
    // }
}