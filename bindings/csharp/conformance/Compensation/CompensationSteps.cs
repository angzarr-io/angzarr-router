using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using Google.Protobuf.WellKnownTypes;
using NUnit.Framework;
using Reqnroll;

namespace Angzarr.Router.Conformance.Compensation;

/// <summary>Step definitions for compensation.feature — compensates entries
/// (qualified and unqualified), compensation stamping and undo routing, over
/// payment and inventory aggregates built through the hand-written aggregate
/// API.</summary>
[Binding]
[Scope(Feature = "Compensation routing")]
public sealed class CompensationSteps
{
    private const string Reserve = "test.counter.Reserve";

    private Router _router = null!;
    private BusinessResponse? _resp;
    private CodedError? _err;

    /// <summary>The (code, rejection_reason) of each rejection a payment
    /// compensator handled.</summary>
    private readonly List<(string Code, string Message)> _seen = new();

    [BeforeScenario]
    public void Before()
    {
        _router = new Router();
        _resp = null;
        _err = null;
        _seen.Clear();
    }

    [AfterScenario]
    public void After() => _router?.Dispose();

    private static Rebuilder<Empty> Stateless() => new(() => new Empty());

    /// <summary>The payment aggregate (domain "payment"): one compensation
    /// handler per (compensates entry, emitted event name) pair, each recording
    /// the rejection's code and message.</summary>
    private void RegisterPayment(params (string Compensates, string Event)[] entries)
    {
        var payment = new AggregateDispatch<Empty>("Payment", "payment", Stateless());
        foreach (var (compensates, ev) in entries)
        {
            payment.OnRejected(
                compensates,
                (n, rejection, state, cctx) =>
                {
                    _seen.Add((rejection.Code, rejection.RejectionReason));
                    return Builders.OneEvent(ev);
                }
            );
        }
        _router.RegisterAggregate(payment);
    }

    private void Dispatch(ContextualCommand cc)
    {
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

    private IList<EventPage> Pages()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed: " + _err?.Message);
        return _resp!.Events?.Pages ?? (IList<EventPage>)new List<EventPage>();
    }

    [Given("a payment aggregate compensating Reserve from any domain with {word}")]
    public void PaymentUnqualified(string ev) => RegisterPayment((Reserve, ev));

    [Given("a second payment aggregate compensating Reserve from any domain with {word}")]
    public void SecondPayment(string ev) => RegisterPayment((Reserve, ev));

    [Given(
        "a payment aggregate compensating Reserve from {string} with {word} and from {string} with {word}"
    )]
    public void PaymentQualified(
        string firstDomain,
        string firstEvent,
        string secondDomain,
        string secondEvent
    ) =>
        RegisterPayment(
            (firstDomain + ":" + Reserve, firstEvent),
            (secondDomain + ":" + Reserve, secondEvent)
        );

    [Given(
        "an inventory aggregate undoing AdjustStock with StockAdjustmentReverted and Reserve with StockReleased"
    )]
    public void Inventory() =>
        _router.RegisterAggregate(
            new AggregateDispatch<Empty>("Inventory", "inventory", Stateless())
                .OnUndo(
                    "test.counter.AdjustStock",
                    (n, compensate, state, cctx) => Builders.OneEvent("StockAdjustmentReverted")
                )
                .OnUndo(Reserve, (n, compensate, state, cctx) => Builders.OneEvent("StockReleased"))
        );

    [When("a rejection of {word} sent to {string} is dispatched to the payment aggregate")]
    public void RejectionSentTo(string command, string domain) =>
        Dispatch(Builders.RejectionSentTo(command, domain, null));

    [When(
        "a rejection of {word} sent to {string} is dispatched to the payment aggregate over history ending at sequence {int}"
    )]
    public void RejectionOverHistory(string command, string domain, int last) =>
        Dispatch(Builders.RejectionSentTo(command, domain, (uint)(last + 1)));

    [When(
        "a rejection of {word} with code {string} and message {string} is dispatched to the payment aggregate"
    )]
    public void RejectionWithCode(string command, string code, string message) =>
        Dispatch(Builders.RejectionWith(command, "inventory", null, code, message));

    [When(
        "a rejection of {word} with no code and message {string} is dispatched to the payment aggregate"
    )]
    public void RejectionWithoutCode(string command, string message) =>
        Dispatch(Builders.RejectionWith(command, "inventory", null, "", message));

    [Then("the compensation handler saw code {string} and message {string}")]
    public void SawCode(string code, string message)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed: " + _err?.Message);
        Assert.That(_seen, Is.EqualTo(new[] { (code, message) }), "(code, message) seen");
    }

    [Then("the compensation handler saw an empty code and message {string}")]
    public void SawNoCode(string message)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed: " + _err?.Message);
        Assert.That(_seen, Is.EqualTo(new[] { ("", message) }), "(code, message) seen");
    }

    [When("a Compensate for {word} is dispatched to the inventory aggregate")]
    public void CompensateFor(string command) => Dispatch(Builders.CompensateFor(command));

    [Then("the aggregate emits one {word} event")]
    public void EmitsOne(string ev)
    {
        var pages = Pages();
        Assert.That(pages.Count, Is.EqualTo(1), "exactly one event");
        Assert.That(
            TypeNames.FromUrl(pages[0].Event.TypeUrl),
            Is.EqualTo("test.counter." + ev),
            "emitted event type"
        );
    }

    [Then("the aggregate emits nothing")]
    public void EmitsNothing() => Assert.That(Pages(), Is.Empty, "no events");

    [Then("the emitted event takes sequence {int}")]
    public void TakesSequence(int seq)
    {
        var pages = Pages();
        Assert.That(pages, Is.Not.Empty, "an emitted event");
        Assert.That(
            pages[0].Header?.SequenceTypeCase,
            Is.EqualTo(PageHeader.SequenceTypeOneofCase.Sequence),
            "an explicit sequence"
        );
        Assert.That((int)pages[0].Header!.Sequence, Is.EqualTo(seq));
    }

    [Then("the aggregates emit {word} at sequence {int} then {word} at sequence {int}")]
    public void EmitInOrder(string first, int firstSeq, string second, int secondSeq)
    {
        var got = new List<string>();
        foreach (var page in Pages())
        {
            Assert.That(
                page.Header?.SequenceTypeCase,
                Is.EqualTo(PageHeader.SequenceTypeOneofCase.Sequence),
                "an explicit sequence"
            );
            got.Add(TypeNames.FromUrl(page.Event.TypeUrl) + "@" + page.Header!.Sequence);
        }
        Assert.That(
            got,
            Is.EqualTo(
                new[]
                {
                    "test.counter." + first + "@" + firstSeq,
                    "test.counter." + second + "@" + secondSeq,
                }
            ),
            "events in registration order with continuing sequences"
        );
    }

    [Then("the dispatch fails with {word} as UNIMPLEMENTED")]
    public void FailsUnimplemented(string code)
    {
        Assert.That(_err, Is.Not.Null, "expected coded error " + code);
        Assert.That(_err!.Code, Is.EqualTo(code));
        Assert.That(_err.Grpc, Is.EqualTo(GrpcCode.Unimplemented));
    }
}
