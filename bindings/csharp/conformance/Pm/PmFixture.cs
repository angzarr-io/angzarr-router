using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Pm;

/// <summary>The conformance OrderProcessManager fixture: the newest trigger
/// reacts with a Reserve command (stamped deferred by the router) plus one fact per rebuilt prior-state
/// event; a rejection injects one process event and escalates, recording the
/// rejection's code and message in Seen.</summary>
internal sealed class PmFixture : TC.OrderProcessManagerAngzarr.OrderProcessManagerHandler
{
    /// <summary>The (code, rejection_reason) of each rejection the Reserve
    /// compensator handled.</summary>
    public List<(string Code, string Message)> Seen { get; } = new();

    public ProcessManagerHandleResponse Increased(
        TC.Increased ev,
        TC.OrderProcessManagerState state,
        Destinations dests,
        Cover? triggerCover
    )
    {
        var resp = new ProcessManagerHandleResponse();
        resp.Commands.Add(Builders.ReserveCommand());
        for (uint i = 0; i < state.Count; i++)
        {
            resp.Facts.Add(Builders.OneFact());
        }
        return resp;
    }

    public void ApplyIncreased(
        TC.OrderProcessManagerState state,
        TC.Increased ev,
        PageContext page
    ) => state.Count += 1;

    public ProcessManagerHandleResponse OnReserveRejected(
        Notification n,
        RejectionNotification rejection,
        TC.OrderProcessManagerState state
    )
    {
        Seen.Add((rejection.Code, rejection.RejectionReason));
        var escalation = new Notification { Cover = new Cover { Domain = "escalated" } };
        var resp = new ProcessManagerHandleResponse { Notification = escalation };
        resp.ProcessEvents.Add(Builders.OneFact());
        return resp;
    }
}
