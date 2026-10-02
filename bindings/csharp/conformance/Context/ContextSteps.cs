using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using Google.Protobuf;
using Google.Protobuf.WellKnownTypes;
using NUnit.Framework;
using Reqnroll;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Context;

/// <summary>Step definitions for context.feature — facts, replay, cover access,
/// PM compensator commands and projector page context, over a ledger aggregate,
/// a reserving process-manager and a tracking projector built through the
/// hand-written API.</summary>
[Binding]
[Scope(Feature = "Facts, replay and handler context")]
public sealed class ContextSteps
{
    private Router _router = null!;
    private readonly List<Cover?> _covers = new();
    private readonly List<(ByteString Root, uint Sequence)> _pages = new();
    private readonly List<uint> _applied = new();
    private EventBook? _facts;
    private TC.CounterState? _replayed;
    private ProcessManagerHandleResponse? _pm;
    private BusinessResponse? _command;
    private CodedError? _factErr;

    [BeforeScenario]
    public void Before()
    {
        _router = new Router();
        _covers.Clear();
        _pages.Clear();
        _applied.Clear();
        _facts = null;
        _replayed = null;
        _pm = null;
        _command = null;
        _factErr = null;
    }

    [AfterScenario]
    public void After() => _router?.Dispose();

    /// <summary>The ledger aggregate (domain "ledger") over CounterState:
    /// Increased folds count += 1 and records the page sequence it applied; a
    /// snapshot loads CounterState; IncreaseBy
    /// records the handled cover and emits one Increased whose cover carries
    /// the ledger's own linkage; the only declared fact,
    /// Increased, is recorded as received and flagged by a CounterState
    /// carrying the count it brings the ledger to.</summary>
    [Given("a ledger aggregate")]
    public void Ledger() =>
        _router.RegisterAggregate(
            new AggregateDispatch<TC.CounterState>(
                "Ledger",
                "ledger",
                new Rebuilder<TC.CounterState>(() => new TC.CounterState())
                    .ApplyWithContext(
                        "test.counter.Increased",
                        (state, ev, page) =>
                        {
                            state.Count += 1;
                            _applied.Add(page.Sequence);
                        }
                    )
                    .WithSnapshot(
                        (state, snapshot) =>
                            state.Count = TC.CounterState.Parser.ParseFrom(snapshot.Value).Count
                    )
            )
                .OnCommand(
                    "test.counter.IncreaseBy",
                    (cmd, state, cctx) =>
                    {
                        _covers.Add(cctx.Cover);
                        return new EventBook
                        {
                            Cover = new Cover { Ext = Builders.LedgerLinkage() },
                            Pages = { new EventPage { Event = Pack.Wrap(new TC.Increased()) } },
                        };
                    }
                )
                .OnFact(
                    "test.counter.Increased",
                    (fact, state) =>
                        new FactRecord(
                            fact,
                            Pack.Wrap(new TC.CounterState { Count = state.Count + 1 })
                        )
                )
        );

    /// <summary>The reserving process-manager (domain "reserving-pm", target
    /// "inventory") over CounterState (Increased folds count += 1): an
    /// Increased trigger from "counter"
    /// records the trigger cover and emits nothing; a rejected Reserve is
    /// compensated with a Release command to "inventory".</summary>
    [Given("a reserving process-manager")]
    public void Reserving() =>
        _router.RegisterProcessManager(
            new ProcessManagerDispatch<TC.CounterState>(
                "Reserving",
                "reserving-pm",
                new[] { "inventory" },
                new Rebuilder<TC.CounterState>(() => new TC.CounterState()).Apply(
                    "test.counter.Increased",
                    (state, ev) => state.Count += 1
                )
            )
                .OnEvent(
                    "counter",
                    "test.counter.Increased",
                    (ev, state, dests, triggerCover) =>
                    {
                        _covers.Add(triggerCover);
                        return new ProcessManagerHandleResponse();
                    }
                )
                .OnRejectedResponse(
                    "test.counter.Reserve",
                    (n, rejection, state) =>
                    {
                        var release = Builders.ReserveCommand();
                        release.Pages[0].Command = new Any
                        {
                            TypeUrl = Builders.TypeUrl("test.counter.Release"),
                        };
                        return new ProcessManagerHandleResponse { Commands = { release } };
                    }
                )
        );

    /// <summary>The tracking projector: every Increased fold records its book's
    /// root and the page's sequence.</summary>
    [Given("a tracking projector")]
    public void Tracking() =>
        _router.RegisterProjector(
            new ProjectorDispatch<Empty>("Tracker", () => new Empty()).OnEvent(
                "test.counter.Increased",
                (projection, ev, page) =>
                    _pages.Add((page.Cover?.Root?.Value ?? ByteString.Empty, page.Sequence))
            )
        );

    [When("{int} Increased facts are handled over {int} prior Increased events")]
    public void IncreasedFacts(int facts, int prior) => HandleFacts("Increased", facts, prior);

    [When("a Reserve fact is handled over no prior events")]
    public void ReserveFact() => HandleFacts("Reserve", 1, 0);

    private void HandleFacts(string fact, int facts, int prior)
    {
        try
        {
            _facts = _router.DispatchFact(Builders.FactsOver(fact, facts, prior));
            _factErr = null;
        }
        catch (CodedError e)
        {
            _factErr = e;
            _facts = null;
        }
    }

    [When("the ledger replays a snapshot of {int} then {int} Increased events")]
    public void Replay(int count, int events)
    {
        var resp = _router.DispatchReplay("ledger", Builders.ReplayOf(count, events));
        Assert.That(
            TypeNames.FromUrl(resp.State.TypeUrl),
            Is.EqualTo(TC.CounterState.Descriptor.FullName),
            "replayed state type"
        );
        _replayed = TC.CounterState.Parser.ParseFrom(resp.State.Value);
    }

    [When("the reserving process-manager replays {int} Increased events")]
    public void PmReplay(int events)
    {
        var resp = _router.DispatchReplay("reserving-pm", Builders.EventsReplayOf(events));
        Assert.That(
            TypeNames.FromUrl(resp.State.TypeUrl),
            Is.EqualTo(TC.CounterState.Descriptor.FullName),
            "replayed state type"
        );
        _replayed = TC.CounterState.Parser.ParseFrom(resp.State.Value);
    }

    [When("an IncreaseBy command for ledger root {string} is dispatched")]
    public void LedgerCommand(string label) =>
        _command = _router.Dispatch(Builders.LedgerCommand(label));

    [When("an IncreaseBy command for ledger root {string} on behalf of a parent is dispatched")]
    public void LedgerCommandWithParent(string label) =>
        _command = _router.Dispatch(Builders.LedgerCommandWithLinkage(label));

    [When(
        "an Increased trigger of counter root {string} at sequence {int} is dispatched to the reserving process-manager"
    )]
    public void ReservingTrigger(string label, int seq) =>
        _pm = _router.DispatchProcessManager(Builders.ReservingTrigger(label, seq));

    [When(
        "a rejection of Reserve sent to {string} at sequence {int} is dispatched to the reserving process-manager"
    )]
    public void ReservingRejection(string domain, int seq) =>
        _pm = _router.DispatchProcessManager(Builders.ReservingRejection(domain, seq));

    [When("Increased events of counter root {string} at sequences {int} and {int} are projected")]
    public void Projected(string label, int first, int second) =>
        _router.DispatchProjector(Builders.TrackedBook(label, first, second));

    [Then("each Increased fact is recorded, flagged by the counts {int} and {int}")]
    public void RecordedAndFlagged(int first, int second)
    {
        Assert.That(_factErr, Is.Null, "fact dispatch failed");
        Assert.That(_facts, Is.Not.Null, "facts were handled");
        var recorded = new List<(string Type, int? Count)>();
        foreach (var page in _facts!.Pages)
        {
            var type = TypeNames.FromUrl(page.Event.TypeUrl);
            int? count =
                type == "test.counter.CounterState"
                    ? (int)TC.CounterState.Parser.ParseFrom(page.Event.Value).Count
                    : null;
            recorded.Add((type, count));
        }
        Assert.That(
            recorded,
            Is.EqualTo(
                new List<(string, int?)>
                {
                    ("test.counter.Increased", null),
                    ("test.counter.CounterState", first),
                    ("test.counter.Increased", null),
                    ("test.counter.CounterState", second),
                }
            )
        );
    }

    [Then("the facts are refused with {word} as INVALID_ARGUMENT")]
    public void FactsRefused(string code)
    {
        Assert.That(_facts, Is.Null, "nothing is recorded");
        Assert.That(_factErr, Is.Not.Null, "the facts are refused");
        Assert.That(_factErr!.Code, Is.EqualTo(code));
        Assert.That(_factErr.Grpc, Is.EqualTo(GrpcCode.InvalidArgument));
    }

    [Then("the replayed state has a count of {int}")]
    public void Replayed(int count) => Assert.That((int)_replayed!.Count, Is.EqualTo(count));

    [Then("the ledger applied Increased events at sequences {int} and {int}")]
    public void AppliedAt(int first, int second) =>
        Assert.That(
            _applied,
            Is.EqualTo(new[] { (uint)first, (uint)second }),
            "applied page sequences"
        );

    private byte[] SingleRoot()
    {
        Assert.That(_covers.Count, Is.EqualTo(1), "the handler ran once");
        var root = _covers[0]?.Root;
        Assert.That(root, Is.Not.Null, "the handler saw a cover with a root");
        return root!.Value.ToByteArray();
    }

    [Then("the recorded event carries the ledger's own linkage")]
    public void LedgerOwnLinkage()
    {
        Assert.That(_command, Is.Not.Null, "a command was dispatched");
        Assert.That(
            _command!.ResultCase,
            Is.EqualTo(BusinessResponse.ResultOneofCase.Events),
            "expected events"
        );
        Assert.That(_command.Events.Pages.Count, Is.EqualTo(1), "recorded events");
        Assert.That(
            _command.Events.Cover?.Ext,
            Is.EqualTo(Builders.LedgerLinkage()),
            "the handler-set linkage is kept over the command's"
        );
    }

    [Then("the ledger handler saw root {string}")]
    public void LedgerSaw(string label) =>
        Assert.That(SingleRoot(), Is.EqualTo(Builders.RootOf(label)));

    [Then("the reserving process-manager saw trigger root {string}")]
    public void PmSaw(string label) =>
        Assert.That(SingleRoot(), Is.EqualTo(Builders.RootOf(label)));

    [Then(
        "the reserving process-manager emits one Release command deferred from source sequence {int}"
    )]
    public void Release(int seq)
    {
        Assert.That(_pm, Is.Not.Null, "dispatched");
        Assert.That(_pm!.Commands.Count, Is.EqualTo(1), "emitted commands");
        var page = _pm.Commands[0].Pages[0];
        Assert.That(TypeNames.FromUrl(page.Command.TypeUrl), Is.EqualTo("test.counter.Release"));
        Assert.That(
            page.Header?.SequenceTypeCase,
            Is.EqualTo(PageHeader.SequenceTypeOneofCase.AngzarrDeferred),
            "the Release command is deferred"
        );
        Assert.That((int)page.Header!.AngzarrDeferred.SourceSeq, Is.EqualTo(seq));
    }

    [Then("the projector saw root {string} at sequences {int} and {int}")]
    public void ProjectorSaw(string label, int first, int second)
    {
        var root = ByteString.CopyFrom(Builders.RootOf(label));
        Assert.That(
            _pages,
            Is.EqualTo(new[] { (root, (uint)first), (root, (uint)second) }),
            "(root, sequence) per fold"
        );
    }
}
