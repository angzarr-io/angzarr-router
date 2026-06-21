using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using NUnit.Framework;
using Reqnroll;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Pm;

/// <summary>Step definitions for process_manager.feature — the
/// OrderProcessManager stateful trigger-side dispatch.</summary>
[Binding]
[Scope(Feature = "Order process-manager dispatch")]
public sealed class PmSteps
{
    private Router _router = null!;
    private ProcessManagerHandleResponse? _resp;
    private CodedError? _err;

    [BeforeScenario]
    public void Before()
    {
        _router = new Router();
        TC.counter_angzarr.RegisterOrderProcessManager(_router, new PmFixture());
        _resp = null;
        _err = null;
    }

    [AfterScenario]
    public void After() => _router?.Dispose();

    private void Dispatch(ProcessManagerHandleRequest req)
    {
        try
        {
            _resp = _router.DispatchProcessManager(req);
            _err = null;
        }
        catch (CodedError e)
        {
            _err = e;
            _resp = null;
        }
    }

    [Given("an order process-manager")]
    public void AnOrderProcessManager()
    {
        // The fixture is registered in Before.
    }

    [When(
        "an Increased trigger in domain {string} is dispatched with destination inventory sequence {int}"
    )]
    public void IncreasedWithDestination(string domain, int seq) =>
        Dispatch(
            Builders.PmTrigger(
                domain,
                new[] { "test.counter.Increased" },
                null,
                new Dictionary<string, uint> { ["inventory"] = (uint)seq }
            )
        );

    [When("an Increased trigger in domain {string} is dispatched")]
    public void IncreasedInDomain(string domain) =>
        Dispatch(Builders.PmTrigger(domain, new[] { "test.counter.Increased" }, null, null));

    [When("a trigger whose newest page is an undeclared event is dispatched")]
    public void NewestUndeclared() =>
        Dispatch(
            Builders.PmTrigger(
                "counter",
                new[] { "test.counter.Increased", "test.counter.Unwatched" },
                null,
                null
            )
        );

    [When("an Increased trigger is dispatched over a prior state of {int} events")]
    public void IncreasedOverState(int n) =>
        Dispatch(
            Builders.PmTrigger(
                "counter",
                new[] { "test.counter.Increased" },
                Builders.PmStateOf(n),
                null
            )
        );

    [When("a request with no trigger is dispatched")]
    public void NoTrigger() => Dispatch(Builders.PmNoTrigger());

    [When("a trigger with no pages is dispatched")]
    public void EmptyTrigger() => Dispatch(Builders.PmEmptyTrigger());

    [When("a rejection of Reserve is dispatched")]
    public void RejectionReserve() => Dispatch(Builders.PmRejection("test.counter.Reserve"));

    [Then("the process-manager emits one command to {string}")]
    public void EmitsOneCommand(string target)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Commands.Count, Is.EqualTo(1), "emitted commands");
        Assert.That(_resp.Commands[0].Cover.Domain, Is.EqualTo(target), "command target");
    }

    [Then("the command carries destination sequence {int}")]
    public void CommandCarriesSequence(int seq) =>
        Assert.That((int)_resp!.Commands[0].Pages[0].Header.Sequence, Is.EqualTo(seq));

    [Then("the process-manager emits no commands")]
    public void EmitsNoCommands()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Commands.Count, Is.EqualTo(0), "expected no commands");
    }

    [Then("the process-manager rebuilt {int} prior state events")]
    public void RebuiltN(int n)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Facts.Count, Is.EqualTo(n), "rebuilt prior state events");
    }

    [Then("the dispatch fails with {word}")]
    public void DispatchFailsWith(string code)
    {
        Assert.That(_err, Is.Not.Null, "expected coded error " + code);
        Assert.That(_err!.Code, Is.EqualTo(code));
    }

    [Then("the process-manager emits one process event")]
    public void EmitsOneProcessEvent()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.ProcessEvents.Count, Is.EqualTo(1), "process events");
    }

    [Then("the process-manager escalates")]
    public void Escalates()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Notification, Is.Not.Null, "expected an escalation");
    }
}
