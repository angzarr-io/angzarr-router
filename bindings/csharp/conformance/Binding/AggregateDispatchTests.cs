using Angzarr;
using Angzarr.Router;
using NUnit.Framework;
using TC = Test.Counter;

namespace Angzarr.Router.Conformance.Binding;

/// <summary>Hand-written aggregate dispatch through the binding: shapes the
/// generated fixture cannot express (a handler raising an arbitrary coded
/// failure).</summary>
[TestFixture]
public sealed class AggregateDispatchTests
{
    private static AggregateDispatch<TC.CounterState> Counter() =>
        new("Counter", "counter", new Rebuilder<TC.CounterState>(() => new TC.CounterState()));

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
}
