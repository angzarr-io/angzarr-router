using System.Linq;
using Angzarr;
using Angzarr.Router;
using NUnit.Framework;
using Reqnroll;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Pm;

/// <summary>Step definitions for process_manager.feature — the
/// OrderProcessManager stateful trigger-side dispatch, alone or co-resident
/// with the AuditProcessManager on one router.</summary>
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
    public void AnOrderProcessManager() =>
        TC.OrderProcessManagerAngzarr.RegisterOrderProcessManager(_router, new PmFixture());

    [Given("co-resident order and audit process-managers")]
    public void CoResidentOrderAndAudit()
    {
        TC.OrderProcessManagerAngzarr.RegisterOrderProcessManager(_router, new PmFixture());
        TC.AuditProcessManagerAngzarr.RegisterAuditProcessManager(_router, new AuditPmFixture());
    }

    [When("an Increased trigger in domain {string} at sequence {int} is dispatched")]
    public void IncreasedAt(string domain, int seq) =>
        Dispatch(Builders.PmTrigger(domain, new[] { "test.counter.Increased" }, null, (uint)seq));

    [When("a Compensate for Reserve is dispatched to the order process-manager")]
    public void CompensateForReserve() => Dispatch(Builders.PmCompensate("Reserve"));

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

    [When("an Increased trigger is dispatched over a prior {string} state of {int} events")]
    public void IncreasedOverOwnedState(string owner, int n) =>
        Dispatch(
            Builders.PmTrigger(
                "counter",
                new[] { "test.counter.Increased" },
                Builders.PmStateIn(owner, n),
                null
            )
        );

    [When("a rejection of Reserve issued by {string} is dispatched")]
    public void RejectionIssuedBy(string issuer) =>
        Dispatch(Builders.PmIssuedRejection("test.counter.Reserve", issuer));

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

    [Then("the command is deferred from source sequence {int} at command index {int}")]
    public void CommandIsDeferred(int seq, int index) =>
        Steps.AssertDeferred(Succeeded().Commands[0], "counter", seq, index);

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

    private ProcessManagerHandleResponse Succeeded()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed: " + _err?.Message);
        return _resp!;
    }

    private int FactsMarked(bool audit) =>
        Succeeded().Facts.Count(f => (f.Cover?.Domain == AuditPmFixture.Mark) == audit);

    [Then("the order process-manager rebuilt {int} prior state events")]
    public void OrderRebuiltN(int n) =>
        Assert.That(FactsMarked(false), Is.EqualTo(n), "order PM facts = its rebuilt events");

    [Then("the audit process-manager rebuilt {int} prior state events")]
    public void AuditRebuiltN(int n) =>
        Assert.That(FactsMarked(true), Is.EqualTo(n), "audit PM facts = its rebuilt events");

    [Then("the order process-manager did not react")]
    public void OrderDidNotReact()
    {
        Assert.That(Succeeded().Commands.Count, Is.EqualTo(0), "the order PM emitted no command");
        Assert.That(FactsMarked(false), Is.EqualTo(0), "the order PM emitted no fact");
    }

    [Then("only the audit process-manager compensates")]
    public void OnlyAuditCompensates()
    {
        var resp = Succeeded();
        Assert.That(resp.ProcessEvents.Count, Is.EqualTo(1), "exactly one compensation");
        Assert.That(
            resp.ProcessEvents[0].Cover?.Domain,
            Is.EqualTo(AuditPmFixture.Mark),
            "the audit PM compensated"
        );
        Assert.That(resp.Notification, Is.Null, "the order PM did not escalate");
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
