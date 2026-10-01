using System;
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
}
