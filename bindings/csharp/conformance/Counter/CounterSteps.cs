using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using NUnit.Framework;
using Reqnroll;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Counter;

/// <summary>
/// Step definitions for counter.feature — the shared cross-language behavior
/// suite, run against the C# binding via Reqnroll. Only this step layer is new;
/// the feature and the fixture mirror the same suite the Rust harness runs.
/// Scoped to the feature so its steps/hooks never collide with saga/pm/projector.
/// </summary>
[Binding]
[Scope(Feature = "Counter aggregate dispatch")]
public sealed class CounterSteps
{
    private Router _router = null!;
    private EventBook? _prior;
    private BusinessResponse? _resp;
    private CodedError? _err;
    private readonly List<CounterFixture.Observation> _observed = new();

    [BeforeScenario]
    public void Before()
    {
        _observed.Clear();
        _router = new Router();
        TC.CounterAggregateAngzarr.RegisterCounterAggregate(_router, new CounterFixture(_observed));
        _prior = null;
        _resp = null;
        _err = null;
    }

    [AfterScenario]
    public void After() => _router?.Dispose();

    private void Dispatch(ContextualCommand cc)
    {
        if (_prior != null)
        {
            cc.Events = _prior;
        }
        try
        {
            _resp = _router.Dispatch(cc);
            _err = null;
        }
        catch (CodedError e)
        {
            _err = e;
            _resp = null;
        }
    }

    // --- Given: prior history --------------------------------------------------

    [Given("a new counter")]
    public void ANewCounter() => _prior = null;

    [Given("a counter that has already recorded {int} increase(s)")]
    public void RecordedIncreases(int n) => _prior = Builders.PriorIncreases(n);

    [Given("a counter whose history holds a corrupt event")]
    public void HistoryHoldsCorrupt() => _prior = Builders.CorruptHistory();

    [Given("a counter restored from a snapshot of 10 with one newer event")]
    public void RestoredFromSnapshot() => _prior = Builders.SnapshotHistory();

    // --- When: dispatch --------------------------------------------------------

    [When("the operator increases the counter by {int}")]
    public void IncreaseByN(int n) => Dispatch(Builders.IncreaseCommand(n));

    [When("the operator increases the counter by {int} on behalf of a parent")]
    public void IncreaseOnBehalf(int n) => Dispatch(Builders.IncreaseCommandWithLinkage(n));

    [When("the operator triggers a hard failure")]
    public void TriggerHardFailure() => Dispatch(Builders.FailHardCommand());

    [When("an unhandled command is dispatched")]
    public void UnhandledDispatched() => Dispatch(Builders.UnhandledCommand());

    [When("a command with no command book is dispatched")]
    public void CommandNoBook() => Dispatch(Builders.CommandMissingBook());

    [When("a command with an empty command book is dispatched")]
    public void CommandEmptyBook() => Dispatch(Builders.CommandMissingPage());

    [When("a command whose page carries no payload is dispatched")]
    public void CommandNoPayload() => Dispatch(Builders.CommandMissingPayload());

    [When("a Reserve command is rejected")]
    public void ReserveRejected() => Dispatch(Builders.RejectionCommand("test.counter.Reserve"));

    [When("an unregistered command is rejected")]
    public void UnregisteredRejected() =>
        Dispatch(Builders.RejectionCommand("test.counter.Undeclared"));

    // --- Then: assertions ------------------------------------------------------

    [Then("{int} increases are recorded, starting at sequence {int}")]
    public void RecordedStartingAt(int count, int start) => Recorded(count, start);

    [Then("{int} increases are recorded, continuing from sequence {int}")]
    public void RecordedContinuingFrom(int count, int start) => Recorded(count, start);

    private void Recorded(int count, int start)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        var book = _resp!.Events;
        Assert.That(book.Pages.Count, Is.EqualTo(count), "recorded events");
        for (var i = 0; i < count; i++)
        {
            Assert.That(
                (int)book.Pages[i].Header.Sequence,
                Is.EqualTo(start + i),
                $"event {i} sequence"
            );
        }
    }

    [Then("the command is rejected as {word}")]
    public void RejectedAs(string code) => FailsWith(code);

    [Then("the command fails with {word}")]
    public void FailsWithCode(string code) => FailsWith(code);

    private void FailsWith(string code)
    {
        Assert.That(_err, Is.Not.Null, "expected coded error " + code);
        Assert.That(_err!.Code, Is.EqualTo(code));
    }

    [Then("no events are recorded")]
    public void NoEventsRecorded()
    {
        // After a rejection the response is absent — that is zero events (the
        // dispatch returns null in C#/Java where Go returns a nil-safe zero).
        var pages = _resp == null ? 0 : _resp.Events.Pages.Count;
        Assert.That(pages, Is.EqualTo(0), "expected no events");
    }

    [Then("the recorded events carry the parent linkage")]
    public void EventsCarryParentLinkage()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(
            _resp!.Events.Cover.Ext,
            Is.EqualTo(Builders.ParentLinkage()),
            "cover ext = parent linkage"
        );
    }

    [Then("the compensations run first then second")]
    public void CompensationsFirstThenSecond()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        var book = _resp!.Events;
        var want = new[] { "test.counter.CompensatedFirst", "test.counter.CompensatedSecond" };
        Assert.That(book.Pages.Count, Is.EqualTo(want.Length), "compensation events");
        for (var i = 0; i < want.Length; i++)
        {
            Assert.That(
                FqFromUrl(book.Pages[i].Event.TypeUrl),
                Is.EqualTo(want[i]),
                $"compensation {i}"
            );
        }
    }

    [Then("no compensation is recorded")]
    public void NoCompensation()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        // Success with nothing emitted: the singular Events message field is null
        // in C# (Java's getter returns a default instance; Go a nil-safe zero).
        Assert.That(_resp!.Events?.Pages.Count ?? 0, Is.EqualTo(0), "expected no compensation");
    }

    [Then("the handler saw prior history, at next sequence {int}")]
    public void HandlerSawPriorHistory(int nextSeq) => AssertHistory(true, nextSeq);

    [Then("the handler saw no prior history, at next sequence {int}")]
    public void HandlerSawNoPriorHistory(int nextSeq) => AssertHistory(false, nextSeq);

    private void AssertHistory(bool wantPrior, int nextSeq)
    {
        var obs = LastObserved();
        Assert.That(obs.HadPriorEvents, Is.EqualTo(wantPrior), "HadPriorEvents");
        Assert.That(obs.NextSequence, Is.EqualTo(nextSeq), "NextSequence");
    }

    [Then("the handler saw a counter of {int}, at next sequence {int}")]
    public void HandlerSawCounter(int count, int nextSeq)
    {
        var obs = LastObserved();
        Assert.That(obs.Count, Is.EqualTo(count), "observed counter");
        Assert.That(obs.NextSequence, Is.EqualTo(nextSeq), "NextSequence");
    }

    private CounterFixture.Observation LastObserved()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_observed, Is.Not.Empty, "the handler recorded no observation");
        return _observed[^1];
    }

    private static string FqFromUrl(string url)
    {
        var i = url.LastIndexOf('/');
        return i >= 0 ? url[(i + 1)..] : url;
    }
}
