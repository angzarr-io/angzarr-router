using Angzarr;
using Angzarr.Router;
using NUnit.Framework;
using Reqnroll;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Projector;

/// <summary>Step definitions for projector.feature — the CounterProjector
/// read-side fold.</summary>
[Binding]
[Scope(Feature = "Counter projector dispatch")]
public sealed class ProjectorSteps
{
    private Router _router = null!;
    private Projection? _proj;
    private CodedError? _err;

    [BeforeScenario]
    public void Before()
    {
        _router = new Router();
        TC.counter_angzarr.RegisterCounterProjector(_router, new ProjectorFixture());
        _proj = null;
        _err = null;
    }

    [AfterScenario]
    public void After() => _router?.Dispose();

    private void Dispatch(EventBook book)
    {
        try
        {
            _proj = _router.DispatchProjector(book);
            _err = null;
        }
        catch (CodedError e)
        {
            _err = e;
            _proj = null;
        }
    }

    [Given("a counter projection")]
    public void ACounterProjection()
    {
        // The fixture is registered in Before.
    }

    [When("{int} events are delivered in domain {string}")]
    public void EventsDelivered(int n, string domain) => Dispatch(Builders.DeliveryBook(domain, n));

    [When("a delivery arrives with no cover")]
    public void DeliveryNoCover() => Dispatch(Builders.DeliveryNoCover());

    [Then("the projection records {int} events")]
    public void RecordsCount(int n)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That((int)_proj!.Sequence, Is.EqualTo(n), "projection records");
    }

    [Then("the projection records nothing")]
    public void RecordsNothing()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That((int)_proj!.Sequence, Is.EqualTo(0), "projection records nothing");
    }

    [Then("the delivery fails with {word}")]
    public void DeliveryFailsWith(string code)
    {
        Assert.That(_err, Is.Not.Null, "expected coded error " + code);
        Assert.That(_err!.Code, Is.EqualTo(code));
    }
}
