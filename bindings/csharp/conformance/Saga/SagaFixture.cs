using System;
using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Saga;

/// <summary>The conformance OrderSaga fixture: a declared source event emits a
/// Reserve command stamped with the supplied destination sequence; a rejection
/// injects one fact event.</summary>
internal sealed class SagaFixture : TC.OrderSagaAngzarr.OrderSagaHandler
{
    public SagaEmission Increased(TC.Increased ev, Destinations dests)
    {
        var cmd = Builders.ReserveCommand();
        if (dests.Has("inventory"))
        {
            cmd = dests.StampCommand(cmd, "inventory");
        }
        return new SagaEmission(new[] { cmd }, Array.Empty<EventBook>());
    }

    public IReadOnlyList<EventBook> OnReserveRejected(
        Notification n,
        RejectionNotification rejection
    ) => new[] { Builders.OneFact() };
}
