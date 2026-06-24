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

/// <summary>Folds one event into a projection.</summary>
public delegate void ProjectorEventThunk<TState>(TState projection, Any @event)
    where TState : class, IMessage;

/// <summary>Produces the Projection from the folded projection state.</summary>
public delegate Projection ProjectorFinishThunk<TState>(TState projection, EventBook events)
    where TState : class, IMessage;

/// <summary>Observes events outside the declared fold set.</summary>
public delegate void ProjectorUnknownThunk(string typeUrl);

/// <summary>Translates one source event into a saga emission (stateless).
/// sourceCover is the source book's cover, so the saga can route emitted
/// commands by the trigger's identity (root, ext).</summary>
public delegate SagaEmission SagaEventThunk(Any @event, Destinations dests, Cover sourceCover);

/// <summary>Compensates a rejected command from a saga (stateless).</summary>
public delegate IReadOnlyList<EventBook> SagaRejectionThunk(
    Notification notification,
    RejectionNotification rejection
);

/// <summary>Handles one source event in a process manager.</summary>
public delegate ProcessManagerHandleResponse PmEventThunk<TState>(
    Any @event,
    TState state,
    Destinations dests
)
    where TState : class, IMessage;

/// <summary>Compensates a rejected command from a process manager.</summary>
public delegate PmRejection PmRejectionThunk<TState>(
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

/// <summary>A PM rejection's result: process events to fold + an optional
/// escalation notification (null for none).</summary>
public sealed record PmRejection(IReadOnlyList<EventBook> ProcessEvents, Notification? Escalation);
