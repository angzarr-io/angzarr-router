using Angzarr;
using Angzarr.Router;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Pm;

/// <summary>The conformance AuditProcessManager fixture, co-resident with the
/// order PM over the same trigger and rejected command but over its own state
/// type: it reacts with one "audit" fact per rebuilt prior-state event and no
/// commands, and compensates with one "audit" process event and no
/// escalation.</summary>
internal sealed class AuditPmFixture : TC.AuditProcessManagerAngzarr.AuditProcessManagerHandler
{
    /// <summary>Cover domain stamped on the audit PM's facts and process
    /// events.</summary>
    internal const string Mark = "audit";

    public ProcessManagerHandleResponse Increased(
        TC.Increased ev,
        TC.AuditProcessManagerState state,
        Destinations dests,
        Cover? triggerCover
    )
    {
        var resp = new ProcessManagerHandleResponse();
        foreach (var _ in state.Seen)
        {
            resp.Facts.Add(AuditBook());
        }
        return resp;
    }

    public void ApplyIncreased(
        TC.AuditProcessManagerState state,
        TC.Increased ev,
        PageContext page
    ) => state.Seen.Add("Increased");

    public ProcessManagerHandleResponse OnReserveRejected(
        Notification n,
        RejectionNotification rejection,
        TC.AuditProcessManagerState state
    ) => new() { ProcessEvents = { AuditBook() } };

    private static EventBook AuditBook() => new() { Cover = new Cover { Domain = Mark } };
}
