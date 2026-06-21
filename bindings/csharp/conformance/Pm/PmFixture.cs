using Angzarr;
using Angzarr.Router;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Pm;

/// <summary>The conformance OrderProcessManager fixture: the newest trigger
/// reacts with a stamped Reserve command plus one fact per rebuilt prior-state
/// event; a rejection injects one process event and escalates.</summary>
internal sealed class PmFixture : TC.counter_angzarr.OrderProcessManagerHandler
{
    public ProcessManagerHandleResponse Increased(
        TC.Increased ev,
        TC.OrderProcessManagerState state,
        Destinations dests
    )
    {
        var cmd = Builders.ReserveCommand();
        if (dests.Has("inventory"))
        {
            cmd = dests.StampCommand(cmd, "inventory");
        }
        var resp = new ProcessManagerHandleResponse();
        resp.Commands.Add(cmd);
        for (uint i = 0; i < state.Count; i++)
        {
            resp.Facts.Add(Builders.OneFact());
        }
        return resp;
    }

    public void ApplyIncreased(TC.OrderProcessManagerState state, TC.Increased ev) =>
        state.Count += 1;

    public PmRejection OnReserveRejected(
        Notification n,
        RejectionNotification rejection,
        TC.OrderProcessManagerState state
    )
    {
        var escalation = new Notification { Cover = new Cover { Domain = "escalated" } };
        return new PmRejection(new[] { Builders.OneFact() }, escalation);
    }
}
