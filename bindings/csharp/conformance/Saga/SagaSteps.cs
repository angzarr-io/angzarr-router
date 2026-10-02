using System;
using System.Collections.Generic;
using System.IO;
using System.Security.Cryptography;
using Angzarr;
using Angzarr.Router;
using Google.Protobuf;
using Io.Angzarr.Router.Ffi.V1;
using NUnit.Framework;
using Reqnroll;

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
    private int? _registration;
    private CodedError? _registrationErr;
    private readonly List<uint> _seen = new();

    [BeforeScenario]
    public void Before()
    {
        _router = new Router();
        _resp = null;
        _err = null;
        _registration = null;
        _registrationErr = null;
        _seen.Clear();
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
    public void AnOrderSaga(string target) =>
        _router.RegisterSaga(SagaFixture.Recording(new SagaFixture(), target, _seen));

    [When("an Increased event at sequence {int} is dispatched")]
    public void IncreasedAt(int seq) =>
        Dispatch(Builders.SagaEventSource("test.counter.Increased", (uint)seq));

    [When("a Reserve event is dispatched")]
    public void ReserveEvent() => Dispatch(Builders.SagaEventSource("test.counter.Reserve", null));

    [When("a source with no pages is dispatched")]
    public void SourceNoPages() => Dispatch(Builders.SagaSourceNoPages());

    [When("a request with no source is dispatched")]
    public void RequestNoSource() => Dispatch(Builders.SagaRequestNoSource());

    [When("a rejection of Reserve is dispatched")]
    public void RejectionReserve() =>
        Dispatch(Builders.SagaRejectionSource("test.counter.Reserve"));

    [Then("the saga emits one command to {string}")]
    public void EmitsOneCommand(string target)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Commands.Count, Is.EqualTo(1), "emitted commands");
        Assert.That(_resp.Commands[0].Cover.Domain, Is.EqualTo(target), "command target");
    }

    [Then("the command is deferred from source sequence {int} at command index {int}")]
    public void CommandIsDeferred(int seq, int index)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Steps.AssertDeferred(_resp!.Commands[0], "order", seq, index);
    }

    [Given("a parity saga emitting the parity command twice")]
    public void AParitySaga() =>
        _router.RegisterSaga(
            new SagaDispatch("parity-saga", "order", new[] { "inventory" }).OnEvent(
                "test.counter.Increased",
                (ev, dests, sourceCover) =>
                    new SagaEmission(
                        new[] { Builders.ParityCommand(), Builders.ParityCommand() },
                        Array.Empty<EventBook>()
                    )
            )
        );

    [When("an Increased event of order root {string} at sequence {int} is dispatched")]
    public void RootedIncreased(string label, int seq) =>
        Dispatch(Builders.SagaRootedSource(label, (uint)seq));

    [When("the parity source event at sequence {int} is dispatched")]
    public void ParitySource(int seq) => Dispatch(Builders.ParitySource((uint)seq));

    /// <summary>Registers a hand-built SagaDescriptor declaring a Reserve
    /// rejection handler through the binding's raw FFI registration (the
    /// SagaDispatch API cannot declare one) on a fresh native router.</summary>
    [When("a saga declaring a compensation for Reserve is registered")]
    public void RegisterCompensatingSaga()
    {
        var descriptor = new SagaDescriptor
        {
            Name = "order-saga",
            InputDomain = "order",
            TargetDomains = { "inventory" },
            Rejections =
            {
                new RejectionEntry { Compensates = "test.counter.Reserve", CallbackIds = { 1UL } },
            },
        }.ToByteArray();
        using var handle = Ffi.RouterNew();
        var ret = Ffi.RegisterSaga(handle, descriptor);
        _registration = ret;
        _registrationErr = ret == 0 ? null : Statuses.FromStatusBytes(null, ret);
    }

    [Then("the registration is refused as INVALID_ARGUMENT")]
    public void RegistrationRefused()
    {
        Assert.That(_registration, Is.EqualTo(-3), "register returns -INVALID_ARGUMENT");
        Assert.That(_registrationErr, Is.Not.Null, "the binding surfaces a coded error");
        Assert.That(_registrationErr!.Grpc, Is.EqualTo(GrpcCode.InvalidArgument), "grpc");
    }

    [Then("the command leaves its source component to the coordinator")]
    public void NoSourceComponent()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Steps.AssertNoSourceComponent(_resp!.Commands[0]);
    }

    [Then("the command is deferred from order root {string}")]
    public void DeferredFromRoot(string label)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Commands.Count, Is.EqualTo(1), "emitted commands");
        foreach (var page in _resp.Commands[0].Pages)
        {
            Assert.That(
                page.Header?.SequenceTypeCase,
                Is.EqualTo(PageHeader.SequenceTypeOneofCase.AngzarrDeferred),
                "command page is deferred"
            );
            Assert.That(
                page.Header!.AngzarrDeferred.Source,
                Is.EqualTo(Builders.CoverOf("order", label)),
                "the source is the triggering book's whole cover"
            );
        }
    }

    [Then("the command at index {int} hashes to SHA-256 {string}")]
    public void CommandHash(int index, string hash)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(Sha256Hex(_resp!.Commands[index]), Is.EqualTo(hash));
    }

    /// <summary>Lowercase hex SHA-256 of the command's deterministic encoding,
    /// unknown fields discarded.</summary>
    private static string Sha256Hex(CommandBook emitted)
    {
        var command = CommandBook
            .Parser.WithDiscardUnknownFields(true)
            .ParseFrom(emitted.ToByteArray());
        using var bytes = new MemoryStream();
        using (var output = new CodedOutputStream(bytes, leaveOpen: true))
        {
            command.WriteTo(output);
        }
        return Convert.ToHexString(SHA256.HashData(bytes.ToArray())).ToLowerInvariant();
    }

    [Then("the saga handler saw source sequence {int}")]
    public void HandlerSawSequence(int seq)
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_seen, Is.EqualTo(new[] { (uint)seq }), "source sequences the handler saw");
    }

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

    [Then("the saga injects no events")]
    public void InjectsNoEvents()
    {
        Assert.That(_err, Is.Null, "dispatch unexpectedly failed");
        Assert.That(_resp!.Events.Count, Is.EqualTo(0), "expected no events");
    }
}
