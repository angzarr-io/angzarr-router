using System;
using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using NUnit.Framework;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Binding;

/// <summary>Destinations are the component's declared output domains: a saga's
/// target domains, a process manager's target domains — never anything the
/// request carries.</summary>
[TestFixture]
public sealed class DestinationsTests
{
    [Test]
    public void DestinationsReportTheDeclaredDomainsInOrder()
    {
        var dests = new Destinations(new[] { "inventory", "billing" });
        Assert.That(dests.Domains(), Is.EqualTo(new[] { "inventory", "billing" }));
        Assert.That(dests.Has("billing"), Is.True);
        Assert.That(dests.Has("order"), Is.False);
        Assert.That(new Destinations(null).Domains(), Is.Empty);
    }

    [Test]
    public void ASagaHandlerSeesItsTargetDomains()
    {
        IReadOnlyList<string>? seen = null;
        var saga = new SagaDispatch("s", "order", "inventory", "billing").OnEvent(
            "test.counter.Increased",
            (ev, dests, cover) =>
            {
                seen = dests.Domains();
                return new SagaEmission(
                    new[] { Builders.ReserveCommand() },
                    Array.Empty<EventBook>()
                );
            }
        );
        using var router = new Router();
        router.RegisterSaga(saga);

        var resp = router.DispatchSaga(Builders.SagaEventSource("test.counter.Increased", 3));

        Assert.That(seen, Is.EqualTo(new[] { "inventory", "billing" }));
        Steps.AssertDeferred(resp.Commands[0], "order", 3, 0);
    }

    [Test]
    public void AProcessManagerHandlerSeesItsTargetDomains()
    {
        IReadOnlyList<string>? seen = null;
        var pm = new ProcessManagerDispatch<TC.CounterState>(
            "p",
            "p-pm",
            new[] { "inventory" },
            new Rebuilder<TC.CounterState>(() => new TC.CounterState())
        ).OnEvent(
            "counter",
            "test.counter.Increased",
            (ev, state, dests) =>
            {
                seen = dests.Domains();
                return new ProcessManagerHandleResponse
                {
                    Commands = { Builders.ReserveCommand() },
                };
            }
        );
        using var router = new Router();
        router.RegisterProcessManager(pm);

        var resp = router.DispatchProcessManager(
            Builders.PmTrigger("counter", new[] { "test.counter.Increased" }, null, 9)
        );

        Assert.That(seen, Is.EqualTo(new[] { "inventory" }));
        Steps.AssertDeferred(resp.Commands[0], "counter", 9, 0);
    }

    [Test]
    public void AProcessManagerDeclaringNoTargetsSeesNone()
    {
        IReadOnlyList<string>? seen = null;
        var pm = new ProcessManagerDispatch<TC.CounterState>(
            "p",
            "p-pm",
            new Rebuilder<TC.CounterState>(() => new TC.CounterState())
        ).OnEvent(
            "counter",
            "test.counter.Increased",
            (ev, state, dests) =>
            {
                seen = dests.Domains();
                return new ProcessManagerHandleResponse();
            }
        );
        using var router = new Router();
        router.RegisterProcessManager(pm);

        router.DispatchProcessManager(
            Builders.PmTrigger("counter", new[] { "test.counter.Increased" }, null, null)
        );

        Assert.That(seen, Is.Not.Null.And.Empty);
    }
}
