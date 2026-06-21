using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using NUnit.Framework;
using Reqnroll;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Saga;

/// <summary>Step definitions for saga.feature — the OrderSaga translation-side
/// dispatch. Scoped to the feature (saga and pm share step text).</summary>
[Binding]
[Scope(Feature = "Order saga dispatch")]
public sealed class SagaSteps
{
    private Router _router = null!;
    private SagaResponse? _resp;
    private CodedError? _err;

    [BeforeScenario]
    public void Before()
    {
        _router = new Router();
        TC.counter_angzarr.RegisterOrderSaga(_router, new SagaFixture());
        _resp = null;
        _err = null;
    }

    [AfterScenario]
    public void After() => _router?.Dispose();

    private void Dispatch(SagaHandleRequest req)
    {
        try
        {
            _resp = _router.DispatchSaga(req);
            _err = null;
        }
        catch (CodedError e)
        {
            _err = e;
            _resp = null;
        }
    }

    [Given("an order saga delivering to {string}")]
    public void AnOrderSaga(string target)
    {
        // The fixture is registered in Before; the delivery target ("inventory")
        // is part of the declaration the generated wiring already carries.
    }

    [When("an Increased event is dispatched with destination inventory sequence {int}")]
    public void IncreasedWithDestination(int seq) =>
        Dispatch(
            Builders.SagaEventSource(
                "test.counter.Increased",
                new Dictionary<string, uint> { ["inventory"] = (uint)seq }
            )
        );

    [When("a Reserve event is dispatched")]
    public void ReserveEvent() => Dispatch(Builders.SagaEventSource("test.counter.Reserve", null));

    [When("a source with no pages is dispatched")]
    public void SourceNoPages() => Dispatch(Builders.SagaSourceNoPages());

    [When("a request with no source is dispatched")]
    public void RequestNoSource() => Dispatch(Builders.SagaRequestNoSource());

    [When("a rejection of Reserve is dispatched")]
    public void RejectionReserve() =>
        Dispatch(Builders.SagaRejectionSource("test.counter.Reserve"));

    [When("a rejection of Unwatched is dispatched")]
    public void RejectionUnwatched() =>
        Dispatch(Builders.SagaRejectionSource("test.counter.Unwatched"));

    [Then("the saga emits one command to {string}")]
    public void EmitsOneCommand(string target)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Commands.Count, Is.EqualTo(1), "emitted commands");
        Assert.That(_resp.Commands[0].Cover.Domain, Is.EqualTo(target), "command target");
    }

    [Then("the command carries destination sequence {int}")]
    public void CommandCarriesSequence(int seq) =>
        Assert.That((int)_resp!.Commands[0].Pages[0].Header.Sequence, Is.EqualTo(seq));

    [Then("the saga emits no commands")]
    public void EmitsNoCommands()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Commands.Count, Is.EqualTo(0), "expected no commands");
    }

    [Then("the dispatch fails with {word}")]
    public void DispatchFailsWith(string code)
    {
        Assert.That(_err, Is.Not.Null, "expected coded error " + code);
        Assert.That(_err!.Code, Is.EqualTo(code));
    }

    [Then("the saga injects one fact event")]
    public void InjectsOneEvent()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Events.Count, Is.EqualTo(1), "injected events");
    }

    [Then("the saga injects no events")]
    public void InjectsNoEvents()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Events.Count, Is.EqualTo(0), "expected no events");
    }
}
