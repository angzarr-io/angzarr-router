using System.Collections.Generic;
using Google.Protobuf;
using Google.Protobuf.WellKnownTypes;

namespace Angzarr.Router;

// The typed business thunks the dispatch builders hold and the generated wiring
// provides. The stateful thunks are generic in the component's state message
// (TState) — the generated wiring is cast-free; the single erasing cast lives in
// the Router's invoker adapters. A thunk throws to fail — the trampoline catches
// and codes it.

/// <summary>Folds one event into rebuilding state.</summary>
public delegate void ApplierThunk<TState>(TState state, Any @event)
    where TState : class, IMessage;

/// <summary>Handles a command; returns the EventBook to persist, or null for
/// nothing emitted.</summary>
public delegate EventBook? CommandThunk<TState>(Any command, TState state, CommandContext cctx)
    where TState : class, IMessage;

/// <summary>Compensates a rejected command; returns a BusinessResponse, or null
/// for nothing.</summary>
public delegate BusinessResponse? RejectionThunk<TState>(
    Notification notification,
    RejectionNotification rejection,
    TState state,
    CommandContext cctx
)
    where TState : class, IMessage;

/// <summary>Undoes an executed command (a Compensate routed by its command
/// type); returns a BusinessResponse, or null for nothing.</summary>
public delegate BusinessResponse? UndoThunk<TState>(
    Notification notification,
    Compensate compensate,
    TState state,
    CommandContext cctx
)
    where TState : class, IMessage;

/// <summary>Handles one fact against the rebuilt state; returns the fact to
/// record (an annotation), or null to record it unchanged. A fact is an
/// external reality: the handler may annotate it but never refuse it.</summary>
public delegate Any? FactThunk<TState>(Any fact, TState state)
    where TState : class, IMessage;

/// <summary>Folds one event into a projection.</summary>
public delegate void ProjectorEventThunk<TState>(TState projection, Any @event)
    where TState : class, IMessage;

/// <summary>Folds one event into a projection, knowing where it sits (its
/// book's cover and the page's sequence).</summary>
public delegate void ProjectorPageThunk<TState>(TState projection, Any @event, PageContext page)
    where TState : class, IMessage;

/// <summary>Produces the Projection from the folded projection state.</summary>
public delegate Projection ProjectorFinishThunk<TState>(TState projection, EventBook events)
    where TState : class, IMessage;

/// <summary>Observes events outside the declared fold set.</summary>
public delegate void ProjectorUnknownThunk(string typeUrl);

/// <summary>Translates one source event into a saga emission (stateless).
/// sourceCover is the source book's cover, so the saga can route emitted
/// commands by the trigger's identity (root, ext); dests are the saga's declared
/// output domains. Emitted commands are deferred — the router stamps them.</summary>
public delegate SagaEmission SagaEventThunk(Any @event, Destinations dests, Cover sourceCover);

/// <summary>Handles one source event in a process manager.</summary>
public delegate ProcessManagerHandleResponse PmEventThunk<TState>(
    Any @event,
    TState state,
    Destinations dests
)
    where TState : class, IMessage;

/// <summary>Handles one source event in a process manager, reading the trigger
/// book's cover (the workflow's identity: domain, root, ext).</summary>
public delegate ProcessManagerHandleResponse PmTriggerThunk<TState>(
    Any @event,
    TState state,
    Destinations dests,
    Cover? triggerCover
)
    where TState : class, IMessage;

/// <summary>Compensates a rejected command from a process manager with process
/// events and an optional escalation.</summary>
public delegate PmRejection PmRejectionThunk<TState>(
    Notification notification,
    RejectionNotification rejection,
    TState state
)
    where TState : class, IMessage;

/// <summary>Compensates a rejected command from a process manager with a full
/// response: process events, commands (deferred by the router), facts and an
/// optional escalation.</summary>
public delegate ProcessManagerHandleResponse PmCompensatorThunk<TState>(
    Notification notification,
    RejectionNotification rejection,
    TState state
)
    where TState : class, IMessage;

/// <summary>A saga event's emission: commands to issue + fact events to inject.</summary>
public sealed record SagaEmission(
    IReadOnlyList<CommandBook> Commands,
    IReadOnlyList<EventBook> Events
);

/// <summary>Where a folded event sits: its book's cover (null when absent) and
/// the page's explicit sequence (0 when absent).</summary>
public readonly record struct PageContext(Cover? Cover, uint Sequence);

/// <summary>A PM rejection's result: process events to fold + an optional
/// escalation notification (null for none).</summary>
public sealed record PmRejection(IReadOnlyList<EventBook> ProcessEvents, Notification? Escalation);
