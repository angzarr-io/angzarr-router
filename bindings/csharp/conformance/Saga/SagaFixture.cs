using System;
using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Saga;

/// <summary>The conformance OrderSaga fixture: a declared source event emits one
/// Reserve command for "inventory", which the router stamps deferred.</summary>
internal sealed class SagaFixture : TC.OrderSagaAngzarr.OrderSagaHandler
{
    public SagaEmission Increased(TC.Increased ev, Destinations dests, Cover sourceCover) =>
        new(new[] { Builders.ReserveCommand() }, Array.Empty<EventBook>());

    /// <summary>The OrderSaga dispatch delivering to target, built through the
    /// context-taking registration: the Increased handler records each
    /// triggering event's sequence in seen, then runs fixture.</summary>
    internal static SagaDispatch Recording(SagaFixture fixture, string target, List<uint> seen) =>
        new SagaDispatch("OrderSaga", "order", target).OnEventWithContext(
            "test.counter.Increased",
            (eventAny, dests, source) =>
            {
                seen.Add(source.Sequence);
                var ev = CodedError.Parse(TC.Increased.Parser, eventAny);
                return fixture.Increased(ev, dests, source.Cover!);
            }
        );
}
