package se.kth.benchmarks.kompicsjava

import java.net.ServerSocket
import java.util.UUID
import java.util.concurrent.{CountDownLatch, TimeUnit}

import se.kth.benchmarks.kompicsjava.bench.JPartitioningComp
import se.kth.benchmarks.kompicsjava.bench.JPartitioningComp.Run
import se.kth.benchmarks.kompicsjava.bench.atomicregister.AtomicRegister
import se.kth.benchmarks.kompicsjava.broadcast.{BEBComp, BestEffortBroadcast}
import se.kth.benchmarks.kompicsscala.{KompicsSystemProvider, NetAddress}
import se.kth.benchmarks.test.KVTestUtil.KVTimestamp
import se.sics.kompics.network.Network
import se.sics.kompics.network.netty.{NettyInit, NettyNetwork}
import se.sics.kompics.sl.{ComponentDefinition, Kompics, KompicsEvent, Start, handle, Init => KompicsInit}

import scala.collection.immutable.List
import scala.concurrent.Promise

class KVLauncherComp(init: KompicsInit[KVLauncherComp]) extends ComponentDefinition {
  case object StartPartitioning extends KompicsEvent

  val KompicsInit(results_promise: Promise[List[KVTimestamp]] @unchecked,
                  partition_size: Int,
                  num_keys: Long,
                  read_workload: Float,
                  write_workload: Float) = init

  val nodes = for (_ <- 0 to partition_size) yield {
    val port = new ServerSocket(0).getLocalPort
    val addr = NetAddress.from("127.0.0.1", port).get
    val net = create(classOf[NettyNetwork], new NettyInit(addr))
    val beb_comp = create(classOf[BEBComp], new BEBComp.Init(addr.asJava))
    val c = Kompics.config.copy(false).asInstanceOf[se.sics.kompics.config.Config.Impl]
    val cb = c.modify(UUID.randomUUID())
    cb.setValue(KompicsSystemProvider.SELF_ADDR_KEY, addr)
    val cu = cb.finalise()
    val atomicreg_comp = create(classOf[AtomicRegister], new AtomicRegister.Init(read_workload, write_workload, true), cu)
    connect[Network](net -> beb_comp)
    connect[Network](net -> atomicreg_comp)
    connect[BestEffortBroadcast](beb_comp -> atomicreg_comp)
    addr
  }

  ctrl uponEvent {
    case _: Start =>
      handle{
        trigger(StartPartitioning -> onSelf)
      }
  }

  loopbck uponEvent {
    case StartPartitioning =>
      handle {
        val prepare_latch = new CountDownLatch(1)
        val port = new ServerSocket(0).getLocalPort
        val p_comp_addr = NetAddress.from("127.0.0.1", port).get
        val p_comp_net = create(classOf[NettyNetwork], new NettyInit(p_comp_addr))
        val c = Kompics.config.copy(false).asInstanceOf[se.sics.kompics.config.Config.Impl]
        val cb = c.modify(UUID.randomUUID());
        cb.setValue(KompicsSystemProvider.SELF_ADDR_KEY, p_comp_addr)
        val cu = cb.finalise()
        val p_comp = create(classOf[JPartitioningComp], KompicsInit[JPartitioningComp](prepare_latch, None, 1, nodes.toList, num_keys, partition_size, Some(results_promise)), cu)
        connect[Network](p_comp_net -> p_comp)
        trigger(Start -> p_comp_net.control())
        trigger(Start -> p_comp.control())
        prepare_latch.await()
        trigger(Run -> p_comp.control())
      }
  }

}
