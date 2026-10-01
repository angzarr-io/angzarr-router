using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using Google.Protobuf.WellKnownTypes;
using NUnit.Framework;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Binding;

/// <summary>Hand-written aggregate dispatch through the binding: shapes the
/// generated fixture cannot express (several compensators for one rejected
/// command, a handler raising an arbitrary coded failure).</summary>
[TestFixture]
public sealed class AggregateDispatchTests
{
    private const string Reserve = "test.counter.Reserve";

    private static AggregateDispatch<TC.CounterState> Counter() =>
        new("Counter", "counter", new Rebuilder<TC.CounterState>(() => new TC.CounterState()));

    private static BusinessResponse OneEvent(string typeUrl) =>
        new()
        {
            Events = new EventBook
            {
                Pages = { new EventPage { Event = new Any { TypeUrl = typeUrl } } },
            },
        };

    [Test]
    public void TwoCompensatorsRunInRegistrationOrderAndMergeTheirEvents()
    {
        var ran = new List<string>();
        var dispatch = Counter()
            .OnRejected(
                Reserve,
                (n, rej, state, cctx) =>
                {
                    ran.Add("first");
                    return OneEvent("/first");
                }
            )
            .OnRejected(
                Reserve,
                (n, rej, state, cctx) =>
                {
                    ran.Add("second");
                    return OneEvent("/second");
                }
            );
        using var router = new Router();
        router.RegisterAggregate(dispatch);

        var resp = router.Dispatch(Builders.RejectionCommand(Reserve));

        Assert.That(ran, Is.EqualTo(new[] { "first", "second" }), "compensator order");
        Assert.That(resp.ResultCase, Is.EqualTo(BusinessResponse.ResultOneofCase.Events));
        Assert.That(resp.Events.Pages.Count, Is.EqualTo(2), "merged compensation pages");
        Assert.That(resp.Events.Pages[0].Event.TypeUrl, Is.EqualTo("/first"));
        Assert.That(resp.Events.Pages[1].Event.TypeUrl, Is.EqualTo("/second"));
    }

    [Test]
    public void ACodedErrorWithGrpcOkIsRejectedAsInvalidArgument()
    {
        var dispatch = Counter()
            .OnCommand(
                "test.counter.IncreaseBy",
                (cmd, state, cctx) =>
                    throw new CodedError("ZERO_GRPC", "handler said ok", (GrpcCode)0, null)
            );
        using var router = new Router();
        router.RegisterAggregate(dispatch);

        var err = Assert.Throws<CodedError>(() => router.Dispatch(Builders.IncreaseCommand(1)));

        Assert.That(err!.Code, Is.EqualTo("ZERO_GRPC"));
        Assert.That(err.Grpc, Is.EqualTo(GrpcCode.InvalidArgument));
        Assert.That(err.Message, Is.EqualTo("handler said ok"));
    }

    [Test]
    public void ACodedErrorConstructedWithGrpcOkCarriesInvalidArgument() =>
        Assert.That(
            new CodedError("ZERO_GRPC", "m", (GrpcCode)0, null).Grpc,
            Is.EqualTo(GrpcCode.InvalidArgument)
        );

    [Test]
    public void ACompensatorReadsTheCoverItIsHandling()
    {
        Cover? seen = null;
        var dispatch = Counter()
            .OnRejected(
                Reserve,
                (n, rej, state, cctx) =>
                {
                    seen = cctx.Cover;
                    return null;
                }
            );
        using var router = new Router();
        router.RegisterAggregate(dispatch);

        router.Dispatch(Builders.RejectionCommand(Reserve));

        Assert.That(seen?.Domain, Is.EqualTo("counter"));
    }

    [Test]
    public void AnUndoHandlerReturningNothingEmitsNothing()
    {
        Compensate? seen = null;
        var dispatch = new AggregateDispatch<TC.CounterState>(
            "Inventory",
            "inventory",
            new Rebuilder<TC.CounterState>(() => new TC.CounterState())
        ).OnUndo(
            "test.counter.AdjustStock",
            (n, compensate, state, cctx) =>
            {
                seen = compensate;
                return null;
            }
        );
        using var router = new Router();
        router.RegisterAggregate(dispatch);

        var resp = router.Dispatch(Builders.CompensateFor("AdjustStock"));

        Assert.That(seen?.CommandType, Is.EqualTo("test.counter.AdjustStock"));
        Assert.That(seen!.Reason, Is.EqualTo("aborted"));
        Assert.That(resp.Events?.Pages.Count ?? 0, Is.EqualTo(0), "no events");
    }

    [Test]
    public void AGeneratedAggregateReplaysWithoutDeclaringAStatePacker()
    {
        using var router = new Router();
        TC.CounterAggregateAngzarr.RegisterCounterAggregate(
            router,
            new Counter.CounterFixture(new List<Counter.CounterFixture.Observation>())
        );

        var resp = router.DispatchReplay("", Builders.ReplayOf(4, 3));

        Assert.That(TypeNames.FromUrl(resp.State.TypeUrl), Is.EqualTo("test.counter.CounterState"));
        Assert.That(TC.CounterState.Parser.ParseFrom(resp.State.Value).Count, Is.EqualTo(7u));
    }
}
