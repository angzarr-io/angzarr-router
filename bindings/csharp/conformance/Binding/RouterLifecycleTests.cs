using System;
using Angzarr;
using Angzarr.Router;
using NUnit.Framework;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Binding;

/// <summary>Router lifetime: the native router is released exactly once, and a
/// disposed router refuses further use instead of touching freed memory.</summary>
[TestFixture]
public sealed class RouterLifecycleTests
{
    private static AggregateDispatch<TC.CounterState> Counter() =>
        new AggregateDispatch<TC.CounterState>(
            "Counter",
            "counter",
            new Rebuilder<TC.CounterState>(() => new TC.CounterState())
        ).OnCommand("test.counter.IncreaseBy", (cmd, state, cctx) => null);

    [Test]
    public void DisposingTwiceReleasesOnceAndLeavesTheRouterDisposed()
    {
        var router = new Router();
        router.RegisterAggregate(Counter());
        router.Dispose();
        router.Dispose();
        Assert.That(router.IsDisposed, Is.True);
        Assert.Throws<ObjectDisposedException>(() => router.Dispatch(Builders.IncreaseCommand(1)));
    }

    [Test]
    public void DispatchAfterDisposeFailsWithObjectDisposed()
    {
        var router = new Router();
        router.RegisterAggregate(Counter());
        Assert.That(router.IsDisposed, Is.False);
        router.Dispose();
        var e = Assert.Throws<ObjectDisposedException>(() =>
            router.Dispatch(Builders.IncreaseCommand(1))
        );
        Assert.That(e!.ObjectName, Is.EqualTo(nameof(Router)));
        Assert.Throws<ObjectDisposedException>(() =>
            router.DispatchProjector(Builders.DeliveryBook("counter", 1))
        );
        Assert.Throws<ObjectDisposedException>(() =>
            router.DispatchSaga(Builders.SagaEventSource("test.counter.Increased", null))
        );
        Assert.Throws<ObjectDisposedException>(() =>
            router.DispatchProcessManager(Builders.PmEmptyTrigger())
        );
    }

    [Test]
    public void RegisterAfterDisposeFailsWithObjectDisposed()
    {
        var router = new Router();
        router.Dispose();
        Assert.Throws<ObjectDisposedException>(() => router.RegisterAggregate(Counter()));
    }
}
