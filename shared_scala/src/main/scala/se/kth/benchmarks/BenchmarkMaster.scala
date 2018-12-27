package se.kth.benchmarks

import kompics.benchmarks.benchmarks._
import kompics.benchmarks.messages._
import kompics.benchmarks.distributed._
import scala.concurrent.{ ExecutionContext, Future, Promise }
import scala.concurrent.duration._
import scala.util.{ Try, Success, Failure }
import io.grpc.{ Server, ServerBuilder, ManagedChannelBuilder }
import java.util.concurrent.Executors
import com.typesafe.scalalogging.StrictLogging

case class ClientEntry(address: String, port: Int, stub: BenchmarkClientGrpc.BenchmarkClient)

class BenchmarkMaster(
  val runnerPort: Int,
  val masterPort: Int,
  val waitFor:    Int,
  val benchmarks: BenchmarkFactory) extends StrictLogging { self =>
  import BenchmarkRunner.{ MIN_RUNS, MAX_RUNS, RSE_TARGET, measure, resultToTestResult, rse };

  import BenchmarkMaster.{ State, IterationData };

  implicit val benchmarkPool = ExecutionContext.fromExecutor(Executors.newSingleThreadExecutor());
  val serverPool = ExecutionContext.fromExecutor(Executors.newSingleThreadExecutor());

  private var clients = List.empty[ClientEntry];
  private var resultPromiseO: Option[Promise[TestResult]] = None;

  // Atomic
  private val state: State = State.init();

  private object MasterService extends BenchmarkMasterGrpc.BenchmarkMaster {

    override def checkIn(request: ClientInfo): Future[CheckinResponse] = {
      clients ::= clientInfoToEntry(request);
      if (clients.size == waitFor) {
        goReady();
      }
      Future.successful(CheckinResponse())
    }

  }

  private def goReady(): Unit = {
    state cas (State.INIT -> State.READY);
    // TODO deal with outstanding benchmark requests
  }

  private object RunnerService extends BenchmarkRunnerGrpc.BenchmarkRunner {
    def pingPong(request: PingPongRequest): Future[TestResult] = {
      val b = benchmarks.pingpong;
      runBenchmark(b, request)
    }
    def netPingPong(request: PingPongRequest): Future[TestResult] = {
      val b = benchmarks.netpingpong;
      runBenchmark(b, request)
    }
  }

  private[this] var masterServer: Server = null;
  private[this] var runnerServer: Server = null;

  private[benchmarks] def start(): Unit = {
    masterServer = ServerBuilder.forPort(masterPort).addService(
      BenchmarkMasterGrpc.bindService(MasterService, serverPool))
      .build
      .start;
    logger.info(s"Master Server started, listening on $masterPort");
    runnerServer = ServerBuilder.forPort(runnerPort).addService(
      BenchmarkRunnerGrpc.bindService(RunnerService, serverPool))
      .build
      .start;
    logger.info(s"Runner Server started, listening on $runnerPort");
    sys.addShutdownHook {
      System.err.println("*** shutting down gRPC server since JVM is shutting down")
      self.stop()
      System.err.println("*** server shut down")
    }
  }

  private[benchmarks] def stop(): Unit = {
    if (masterServer != null) {
      masterServer.shutdown()
    }
    if (runnerServer != null) {
      runnerServer.shutdown()
    }
  }

  private[benchmarks] def blockUntilShutdown(): Unit = {
    if (masterServer != null) {
      masterServer.awaitTermination()
    }
    if (runnerServer != null) {
      runnerServer.awaitTermination()
    }
  }

  private def runBenchmark(b: Benchmark, msg: scalapb.GeneratedMessage): Future[TestResult] = {
    b.msgToConf(msg) match {
      case Success(c) => {
        state cas (State.READY -> State.RUN);
        logger.info(s"Starting local test ${b.getClass.getCanonicalName}");
        val f = Future {
          val r = BenchmarkRunner.run(b)(c);
          resultToTestResult(r)
        };
        f.onComplete(_ => {
          logger.info(s"Completed local test ${b.getClass.getCanonicalName}");
          state := State.READY;
        }); // no point to check state here, as the exception would be swallowed anyway
        f
      }
      case Failure(e) => Future.failed(e)
    }
  }

  private def runBenchmark(b: DistributedBenchmark, msg: scalapb.GeneratedMessage): Future[TestResult] = {
    b.msgToMasterConf(msg) match {
      case Success(masterConf) => {
        state cas (State.READY -> State.SETUP);
        val rp = Promise.apply[TestResult];
        resultPromiseO = Some(rp);

        logger.info(s"Starting distributed test ${b.getClass.getCanonicalName}");

        Future {
          val master = b.newMaster();
          val clientConf = master.setup(masterConf);
          val clientConfS = b.clientConfToString(clientConf);
          val clientSetup = SetupConfig(b.getClass.getCanonicalName, clientConfS);
          val clientDataRLF = Future.sequence(clients.map(_.stub.setup(clientSetup)));
          val clientDataLF = clientDataRLF.flatMap(l => {
            val lf = l.map(sr => {
              val t = Try {
                assert(sr.success, sr.data);
                b.strToClientData(sr.data);
              } flatten;
              tryToFuture(t)
            });
            Future.sequence(lf)
          });
          def iteration(clientDataL: List[b.ClientData], nRuns: Int, results: List[Double]): Future[IterationData] = {
            if ((nRuns < MAX_RUNS) && (rse(results) > RSE_TARGET)) {
              master.prepareIteration(clientDataL);
              val r = BenchmarkRunner.measure(master.runIteration);
              master.cleanupIteration(false, r);
              val f = Future.sequence(clients.map(_.stub.cleanup(CleanupInfo(false))));
              f.map(_ => IterationData(nRuns + 1, r :: results, false))
            } else {
              state cas (State.RUN -> State.CLEANUP);
              master.cleanupIteration(true, results.head);
              val f = Future.sequence(clients.map(_.stub.cleanup(CleanupInfo(true))));
              f.map(_ => {
                state cas (State.CLEANUP -> State.FINISHED);
                IterationData(nRuns, results, true)
              })
            }
          };
          clientDataLF.map { clientDataL =>
            state cas (State.SETUP -> State.RUN);
            // run until RSE target is met
            def loop(id: IterationData): Unit = {
              if (id.done) {
                val tr = resultToTestResult(Success(id.results));
                rp.success(tr)
              } else {
                val f = iteration(clientDataL, id.nRuns, id.results);
                f.onComplete {
                  case Success(id) => loop(id)
                  case Failure(ex) => rp.failure(ex)
                };
              }
            };
            val f = iteration(clientDataL, 0, List.empty[Double]);
            f.onComplete {
              case Success(id) => loop(id)
              case Failure(ex) => rp.failure(ex)
            };
          };
        }

        val f = rp.future;
        f.onComplete(_ => {
          logger.info(s"Completed distributed test ${b.getClass.getCanonicalName}");
          state := State.READY;
        }); // no point to check state here, as the exception would be swallowed anyway
        f
      }
      case Failure(e) => Future.failed(e)
    }
  }

  private def clientInfoToEntry(ci: ClientInfo): ClientEntry = {
    val channel = ManagedChannelBuilder.forAddress(ci.address, ci.port).usePlaintext().build;
    val stub = BenchmarkClientGrpc.stub(channel);
    ClientEntry(ci.address, ci.port, stub)
  }

  private def tryToFuture[T](t: Try[T]): Future[T] = t match {
    case Success(cd) => Future.successful(cd)
    case Failure(ex) => Future.failed(ex)
  };
}

object BenchmarkMaster {

  def run(waitFor: Int, masterPort: Int, runnerPort: Int, benchF: BenchmarkFactory): Unit = {
    val inst = new BenchmarkMaster(runnerPort, masterPort, waitFor, benchF);
    inst.start();
    inst.blockUntilShutdown();
  }

  class State(initial: Int) {
    private val _inner: java.util.concurrent.atomic.AtomicInteger = new java.util.concurrent.atomic.AtomicInteger(initial);

    def :=(v: Int): Unit = _inner.set(v);
    def cas(oldValue: Int, newValue: Int): Unit = if (!_inner.compareAndSet(oldValue, newValue)) {
      throw new RuntimeException(s"Invalid State Transition from $oldValue -> $newValue")
    };
    def cas(t: (Int, Int)): Unit = cas(t._1, t._2);
    def apply(): Int = _inner.get;
  }
  object State {
    val INIT = 0;
    val READY = 1;
    val SETUP = 2;
    val RUN = 3;
    val CLEANUP = 4;
    val FINISHED = 5;

    def apply(v: Int): State = new State(v);
    def init(): State = apply(INIT);
  }

  case class IterationData(nRuns: Int, results: List[Double], done: Boolean)
}