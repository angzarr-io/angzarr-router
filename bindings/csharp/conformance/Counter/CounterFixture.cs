using System;
using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using Google.Protobuf.WellKnownTypes;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Counter;

/// <summary>
/// The conformance CounterAggregate fixture, implementing the angzarr-generated
/// handler seam. The behaviour is the contract the shared feature asserts; the
/// wiring is generated, so this fixture is the proof the generated seam is
/// faithful.
/// </summary>
internal sealed class CounterFixture : TC.counter_angzarr.CounterAggregateHandler
{
    /// <summary>The historical-state evidence a command handler saw — what the
    /// suite asserts, since state never crosses the boundary.</summary>
    internal readonly record struct Observation(bool HadPriorEvents, long NextSequence, long Count);

    private readonly List<Observation> _observed;

    internal CounterFixture(List<Observation> observed) => _observed = observed;

    public IReadOnlyList<TC.Increased> IncreaseBy(
        TC.IncreaseBy cmd,
        TC.CounterState state,
        CommandContext cctx
    )
    {
        _observed.Add(new Observation(cctx.HadPriorEvents, cctx.NextSequence, state.Count));
        if (cmd.N == 0)
        {
            throw CodedError.Reject("VALUE_NOT_POSITIVE", "increase amount must be positive");
        }
        var events = new List<TC.Increased>();
        for (uint i = 0; i < cmd.N; i++)
        {
            events.Add(new TC.Increased());
        }
        return events;
    }

    public EventBook FailHard(TC.FailHard cmd, TC.CounterState state, CommandContext cctx) =>
        throw new Exception("hard failure");

    public void ApplyIncreased(TC.CounterState state, TC.Increased ev) => state.Count += 1;

    /// <summary>Appends both ordered markers in one response — the
    /// within-component fan-out collapses to one compensator, preserving the
    /// observable two-marker ordering the feature asserts.</summary>
    public BusinessResponse OnReserveRejected(
        Notification n,
        RejectionNotification rejection,
        TC.CounterState state,
        CommandContext cctx
    ) =>
        new()
        {
            Events = new EventBook
            {
                Pages = { Marker("CompensatedFirst"), Marker("CompensatedSecond") },
            },
        };

    private static EventPage Marker(string name) =>
        new() { Event = new Any { TypeUrl = Builders.TypeUrl("test.counter." + name) } };
}
