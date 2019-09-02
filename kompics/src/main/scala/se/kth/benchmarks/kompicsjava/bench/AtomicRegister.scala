package se.kth.benchmarks.kompicsjava.bench;

import java.util.UUID
import java.util.concurrent.{CountDownLatch, TimeUnit}

import kompics.benchmarks.benchmarks.AtomicRegisterRequest
import se.kth.benchmarks.{ClientEntry, DistributedBenchmark}
import se.kth.benchmarks.kompicsjava.broadcast.{BEBComp, BestEffortBroadcast => JBestEffortBroadcast}
import se.kth.benchmarks.kompicsjava.bench.atomicregister.{AtomicRegister => JAtomicRegister, AtomicRegisterSerializer}
import se.kth.benchmarks.kompicsjava.partitioningcomponent.JPartitioningCompSerializer
import se.kth.benchmarks.kompicsscala._
import se.kth.benchmarks.kompicsscala.bench.AtomicRegister.{ClientParams, FailedPreparationException}
import se.sics.kompics.sl.Init
import se.kth.benchmarks.kompicsjava.bench.JPartitioningComp.Run

import scala.concurrent.Await
import scala.concurrent.duration._
import scala.util.{Success, Try}

object AtomicRegister extends DistributedBenchmark {
  override type MasterConf = AtomicRegisterRequest;
  override type ClientConf = ClientParams;
  override type ClientData = NetAddress;

  class MasterImpl extends Master {
    private var read_workload = 0.0f;
    private var write_workload = 0.0f;
    private var partition_size: Int = -1;
    private var num_keys: Long = -1L;
    private var system: KompicsSystem = null;
    private var atomicRegister: UUID = null;
    private var beb: UUID = null;
    private var partitioning_comp: UUID = null;
    private var prepare_latch: CountDownLatch = null;
    private var finished_latch: CountDownLatch = null;
    private var init_id: Int = -1;

    override def setup(c: MasterConf): ClientConf = {
      println("Atomic Register setup!")
      system = KompicsSystemProvider.newRemoteKompicsSystem(1);
      AtomicRegisterSerializer.register();
      JPartitioningCompSerializer.register();
      this.read_workload = c.readWorkload;
      this.write_workload = c.writeWorkload;
      this.partition_size = c.partitionSize;
      this.num_keys = c.numberOfKeys;
      ClientParams(read_workload, write_workload)
    };
    override def prepareIteration(d: List[ClientData]): Unit = {
      assert(system != null);
      val addr = system.networkAddress.get;
      println(s"Atomic Register(Master) Path is $addr");
      val atomicRegisterIdF =
        system.createNotify[JAtomicRegister](new JAtomicRegister.Init(read_workload, write_workload))
      atomicRegister = Await.result(atomicRegisterIdF, 5.second)
      /* connect network */
      val connF = system.connectNetwork(atomicRegister);
      Await.result(connF, 5.seconds);
      /* connect best effort broadcast */
      val bebF = system.createNotify[BEBComp](new BEBComp.Init(addr.asJava))
      beb = Await.result(bebF, 5.second)
      val beb_net_connF = system.connectNetwork(beb)
      Await.result(beb_net_connF, 5.second)
      val beb_ar_connF = system.connectComponents[JBestEffortBroadcast](atomicRegister, beb)
      Await.result(beb_ar_connF, 5.seconds);
      /* connect Iteration prepare component */
      val nodes = addr :: d
      val num_nodes = nodes.size
      assert(partition_size <= num_nodes && partition_size > 0 && read_workload + write_workload == 1)
      init_id += 1
      prepare_latch = new CountDownLatch(1)
      finished_latch = new CountDownLatch(1)
      val partitioningCompF = system.createNotify[JPartitioningComp](
        Init(prepare_latch, finished_latch, init_id, nodes, num_keys, partition_size)
      )
      partitioning_comp = Await.result(partitioningCompF, 5.second)
      val partitioningComp_net_connF = system.connectNetwork(partitioning_comp)
      Await.result(partitioningComp_net_connF, 5.seconds)
      assert(system != null && beb != null && partitioning_comp != null && atomicRegister != null);
      system.startNotify(beb)
      system.startNotify(atomicRegister)
      system.startNotify(partitioning_comp)
      prepare_latch.await()
      /*val successful_prep = prepare_latch.await(100, TimeUnit.SECONDS)
      if (!successful_prep) {
        println("Timeout on InitAck in prepareIteration")
        throw new FailedPreparationException("Timeout waiting for Init Ack from all nodes")
      }*/
    }
    override def runIteration(): Unit = {
      system.triggerComponent(Run, partitioning_comp)
      finished_latch.await()
    };
    override def cleanupIteration(lastIteration: Boolean, execTimeMillis: Double): Unit = {
      println("Cleaning up Atomic Register(Master) side");
      assert(system != null);
      if (prepare_latch != null) prepare_latch = null
      if (finished_latch != null) finished_latch = null
      if (atomicRegister != null) {
        val killF = system.killNotify(atomicRegister)
        Await.ready(killF, 5.seconds)
        val killBebF = system.killNotify(beb)
        Await.ready(killBebF, 5.seconds)
        val killpartitioningCompF = system.killNotify(partitioning_comp)
        Await.ready(killpartitioningCompF, 5.seconds)
        atomicRegister = null
        beb = null
        partitioning_comp = null
      }
      if (lastIteration) {
        println("Cleaning up Last iteration")
        val f = system.terminate();
        system = null;
      }
    }
  }

  class ClientImpl extends Client {
    private var system: KompicsSystem = null;
    private var atomicRegister: UUID = null;
    private var beb: UUID = null
    private var read_workload = 0.0f
    private var write_workload = 0.0f

    override def setup(c: ClientConf): ClientData = {
      system = KompicsSystemProvider.newRemoteKompicsSystem(1);
      AtomicRegisterSerializer.register();
      JPartitioningCompSerializer.register();
      val addr = system.networkAddress.get;
      println(s"Atomic Register(Client) Path is $addr");
      this.read_workload = c.read_workload;
      this.write_workload = c.write_workload;
      val atomicRegisterIdF =
        system.createNotify[JAtomicRegister](new JAtomicRegister.Init(read_workload, write_workload))
      atomicRegister = Await.result(atomicRegisterIdF, 5.second);
      /* connect network */
      val connF = system.connectNetwork(atomicRegister);
      Await.result(connF, 5.seconds);
      /* connect best effort broadcast */
      val bebF = system.createNotify[BEBComp](new BEBComp.Init(addr.asJava))
      beb = Await.result(bebF, 5.second)
      val beb_net_connF = system.connectNetwork(beb)
      Await.result(beb_net_connF, 5.second)
      val beb_ar_connF = system.connectComponents[JBestEffortBroadcast](atomicRegister, beb)
      Await.result(beb_ar_connF, 5.seconds);
      system.startNotify(beb)
      system.startNotify(atomicRegister)
      addr
    }
    override def prepareIteration(): Unit = {
      // nothing
      println("Preparing Atomic Register(Client) iteration");
      assert(system != null);

    }
    override def cleanupIteration(lastIteration: Boolean): Unit = {
      println("Cleaning up Atomic Register(Client) side");
      assert(system != null);
      if (lastIteration) {
        atomicRegister = null; // will be stopped when system is shut down
        beb = null
        system.terminate();
        system = null;
      }
    }
  }

  override def newMaster(): AtomicRegister.Master = new MasterImpl();

  override def msgToMasterConf(msg: scalapb.GeneratedMessage): Try[MasterConf] = Try {
    msg.asInstanceOf[AtomicRegisterRequest]
  };

  override def newClient(): AtomicRegister.Client = new ClientImpl();

  override def strToClientConf(str: String): Try[ClientConf] = Try {
    val split = str.split(":");
    assert(split.length == 2);
    ClientParams(split(0).toFloat, split(1).toFloat)
  };

  override def strToClientData(str: String): Try[ClientData] =
    Try {
      val split = str.split(":");
      assert(split.length == 2);
      val ipStr = split(0); //.replaceAll("""/""", "");
      val portStr = split(1);
      val port = portStr.toInt;
      NetAddress.from(ipStr, port)
    }.flatten;

  override def clientConfToString(c: ClientConf): String = s"${c.read_workload}:${c.write_workload}";

  override def clientDataToString(d: ClientData): String = {
    s"${d.isa.getHostString()}:${d.getPort()}"
  }

}
