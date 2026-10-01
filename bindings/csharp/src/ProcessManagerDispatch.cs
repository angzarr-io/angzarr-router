using System.Collections.Generic;
using System.Linq;
using Google.Protobuf;

namespace Angzarr.Router;

/// <summary>
/// One process-manager component's registration: its name, its own domain, the
/// output domains it issues commands to, its rebuilder, per-(source-domain,
/// event) handlers, and ordered rejection compensators. A PM is stateful — its
/// appliers fold process state before a handler runs, exactly as an aggregate
/// does. Commands it emits are deferred: the router stamps their provenance.
/// Generic in the state.
/// </summary>
public sealed class ProcessManagerDispatch<TState>
    where TState : class, IMessage
{
    internal readonly string Name;
    internal readonly string PmDomain;
    internal readonly IReadOnlyList<string> Targets;
    internal readonly Rebuilder<TState> Rebuilder;

    // source domain → fully-qualified event type → handler
    internal readonly Dictionary<string, Dictionary<string, PmTriggerThunk<TState>>> Handlers =
        new();
    internal readonly Dictionary<string, List<PmCompensatorThunk<TState>>> Rejections = new();

    /// <summary>A process manager in <paramref name="pmDomain"/> declaring no
    /// output domains.</summary>
    public ProcessManagerDispatch(string name, string pmDomain, Rebuilder<TState> rebuilder)
        : this(name, pmDomain, System.Array.Empty<string>(), rebuilder) { }

    /// <summary>A process manager in <paramref name="pmDomain"/> issuing
    /// commands to the declared <paramref name="targetDomains"/> (its
    /// <see cref="Destinations"/>).</summary>
    public ProcessManagerDispatch(
        string name,
        string pmDomain,
        IEnumerable<string> targetDomains,
        Rebuilder<TState> rebuilder
    )
    {
        Name = name;
        PmDomain = pmDomain;
        Targets = targetDomains.ToList();
        Rebuilder = rebuilder;
    }

    /// <summary>Registers the handler for one source-domain event type.</summary>
    public ProcessManagerDispatch<TState> OnEvent(
        string sourceDomain,
        string fullName,
        PmEventThunk<TState> thunk
    ) => OnEvent(sourceDomain, fullName, (ev, state, dests, _) => thunk(ev, state, dests));

    /// <summary>Registers the handler for one source-domain event type; the
    /// handler also reads the trigger book's cover.</summary>
    public ProcessManagerDispatch<TState> OnEvent(
        string sourceDomain,
        string fullName,
        PmTriggerThunk<TState> thunk
    )
    {
        if (!Handlers.TryGetValue(sourceDomain, out var byType))
        {
            byType = new Dictionary<string, PmTriggerThunk<TState>>();
            Handlers[sourceDomain] = byType;
        }
        byType[fullName] = thunk;
        return this;
    }

    /// <summary>Appends a compensator for one compensates entry
    /// (<c>"fq.Type"</c> or <c>"domain:fq.Type"</c>, passed to the router
    /// verbatim) returning process events and an optional escalation; repeated
    /// calls register an ordered fan-out.</summary>
    public ProcessManagerDispatch<TState> OnRejected(
        string compensates,
        PmRejectionThunk<TState> thunk
    ) =>
        OnRejectedResponse(
            compensates,
            (n, rejection, state) =>
            {
                var r = thunk(n, rejection, state);
                var resp = new ProcessManagerHandleResponse();
                resp.ProcessEvents.AddRange(r.ProcessEvents);
                if (r.Escalation != null)
                {
                    resp.Notification = r.Escalation;
                }
                return resp;
            }
        );

    /// <summary>Appends a compensator for one compensates entry returning a
    /// full response — process events, commands (deferred by the router),
    /// facts and an optional escalation; repeated calls register an ordered
    /// fan-out.</summary>
    public ProcessManagerDispatch<TState> OnRejectedResponse(
        string compensates,
        PmCompensatorThunk<TState> thunk
    )
    {
        if (!Rejections.TryGetValue(compensates, out var list))
        {
            list = new List<PmCompensatorThunk<TState>>();
            Rejections[compensates] = list;
        }
        list.Add(thunk);
        return this;
    }
}
