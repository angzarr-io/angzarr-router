using System.Collections.Generic;
using Angzarr;
using Angzarr.Router;
using Google.Protobuf;
using Google.Protobuf.WellKnownTypes;
using Test.Counter;

namespace Angzarr.Router.Conformance;

/// <summary>
/// The shared conformance envelopes. Google.Protobuf (C#) has no text-format
/// parser, so — unlike the Java/Rust harnesses that parse
/// conformance/fixtures/*.txtpb — these are built BY FIELD, byte-equivalent to
/// those skeletons (each is an orthogonal envelope wrapping an empty inner
/// message; the salient field is set from the scenario). This mirrors how the Go
/// binding constructs its no-skeleton envelopes. The behaviour asserted is the
/// same cross-language contract.
/// </summary>
public static class Builders
{
    /// <summary>The framework's canonical bare-"/" type-URL prefix (the core
    /// keys dispatch on the suffix, so the prefix is immaterial).</summary>
    public static string TypeUrl(string fq) => "/" + fq;

    private static Any AnyOf(string fq, ByteString value) =>
        new() { TypeUrl = TypeUrl(fq), Value = value };

    private static Any AnyEmpty(string fq) => new() { TypeUrl = TypeUrl(fq) };

    // --- commands -----------------------------------------------------------

    /// <summary>The IncreaseBy envelope (command_increase.txtpb) with n set.</summary>
    public static ContextualCommand IncreaseCommand(int n)
    {
        var inner = new IncreaseBy { N = (uint)n };
        return new ContextualCommand
        {
            Command = new CommandBook
            {
                Cover = new Cover { Domain = "counter" },
                Pages =
                {
                    new CommandPage
                    {
                        Command = AnyOf("test.counter.IncreaseBy", inner.ToByteString()),
                    },
                },
            },
        };
    }

    /// <summary>An increase command with parent linkage stamped on its cover.</summary>
    public static ContextualCommand IncreaseCommandWithLinkage(int n)
    {
        var cc = IncreaseCommand(n);
        cc.Command.Cover.Ext = ParentLinkage();
        return cc;
    }

    public static ContextualCommand FailHardCommand() =>
        new()
        {
            Command = new CommandBook
            {
                Cover = new Cover { Domain = "counter" },
                Pages = { new CommandPage { Command = AnyEmpty("test.counter.FailHard") } },
            },
        };

    public static ContextualCommand UnhandledCommand() =>
        new()
        {
            Command = new CommandBook
            {
                Cover = new Cover { Domain = "counter" },
                Pages = { new CommandPage { Command = AnyEmpty("test.counter.Reserve") } },
            },
        };

    /// <summary>Wraps a rejection Notification for fqCommand into a
    /// ContextualCommand — the core detects the notification type and takes the
    /// compensation path.</summary>
    public static ContextualCommand RejectionCommand(string fqCommand)
    {
        var notification = RejectionNotificationFor(fqCommand, "counter");
        return new ContextualCommand
        {
            Command = new CommandBook
            {
                Cover = new Cover { Domain = "counter" },
                Pages = { new CommandPage { Command = Pack.Wrap(notification) } },
            },
        };
    }

    // --- envelope-guard negatives (one structural field cleared) ------------

    public static ContextualCommand CommandMissingBook()
    {
        var cc = IncreaseCommand(1);
        cc.Command = null;
        return cc;
    }

    public static ContextualCommand CommandMissingPage()
    {
        var cc = IncreaseCommand(1);
        cc.Command.Pages.Clear();
        return cc;
    }

    public static ContextualCommand CommandMissingPayload()
    {
        var cc = IncreaseCommand(1);
        cc.Command.Pages[0].Command = null;
        return cc;
    }

    /// <summary>An opaque fill-only ext stamped on a command's cover, used to
    /// prove ext propagation onto emitted events.</summary>
    public static Any ParentLinkage() =>
        AnyOf("test.counter.Parent", ByteString.CopyFrom(new byte[] { 1, 2, 3 }));

    // --- prior history ------------------------------------------------------

    /// <summary>Replays the Increased skeleton at sequences 0..n-1 (null if 0).</summary>
    public static EventBook? PriorIncreases(int n)
    {
        if (n == 0)
        {
            return null;
        }
        var book = new EventBook { NextSequence = (uint)n };
        for (var i = 0; i < n; i++)
        {
            book.Pages.Add(IncreasedPageAt(i));
        }
        return book;
    }

    /// <summary>One Increased page whose payload is an undecodable varint
    /// (PERSISTED_EVENT_CORRUPT on fold).</summary>
    public static EventBook CorruptHistory()
    {
        var page = IncreasedPageAt(0);
        page.Event.Value = ByteString.CopyFrom(new byte[] { 0xff, 0xff, 0xff });
        return new EventBook { Pages = { page }, NextSequence = 1 };
    }

    /// <summary>Seeds count 10 at sequence 10, plus a covered page (10, skipped)
    /// and an uncovered page (11, applied) — a rebuild observes 11.</summary>
    public static EventBook SnapshotHistory() =>
        new()
        {
            Snapshot = new Snapshot
            {
                Sequence = 10,
                State = Pack.Wrap(new CounterState { Count = 10 }),
            },
            Pages = { IncreasedPageAt(10), IncreasedPageAt(11) },
            NextSequence = 12,
        };

    /// <summary>One Increased event page stamped with a sequence.</summary>
    public static EventPage IncreasedPageAt(int seq) =>
        new()
        {
            Event = AnyEmpty("test.counter.Increased"),
            Header = new PageHeader { Sequence = (uint)seq },
        };

    // --- saga / process-manager shared fixtures -----------------------------

    /// <summary>The one-page Reserve command the saga and PM emit for
    /// "inventory".</summary>
    public static CommandBook ReserveCommand() =>
        new()
        {
            Cover = new Cover { Domain = "inventory" },
            Pages = { new CommandPage { Command = AnyEmpty("test.counter.Reserve") } },
        };

    /// <summary>A single empty fact-event book the compensators inject.</summary>
    public static EventBook OneFact() => new() { Pages = { new EventPage() } };

    private static Notification RejectionNotificationFor(string fqCommand, string domain)
    {
        var rejection = new RejectionNotification
        {
            RejectedCommand = new CommandBook
            {
                Cover = new Cover { Domain = domain },
                Pages = { new CommandPage { Command = AnyEmpty(fqCommand) } },
            },
        };
        return new Notification { Payload = Pack.Wrap(rejection) };
    }

    // --- saga dispatch requests ---------------------------------------------

    public static SagaHandleRequest SagaEventSource(
        string fq,
        IReadOnlyDictionary<string, uint>? dest
    )
    {
        var req = new SagaHandleRequest
        {
            Source = new EventBook
            {
                Cover = new Cover { Domain = "order" },
                Pages = { new EventPage { Event = AnyEmpty(fq) } },
            },
        };
        if (dest != null)
        {
            foreach (var kv in dest)
            {
                req.DestinationSequences.Add(kv.Key, kv.Value);
            }
        }
        return req;
    }

    public static SagaHandleRequest SagaRejectionSource(string fqCommand)
    {
        var notification = RejectionNotificationFor(fqCommand, "inventory");
        return new SagaHandleRequest
        {
            Source = new EventBook
            {
                Cover = new Cover { Domain = "order" },
                Pages = { new EventPage { Event = Pack.Wrap(notification) } },
            },
        };
    }

    public static SagaHandleRequest SagaSourceNoPages() => new() { Source = new EventBook() };

    public static SagaHandleRequest SagaRequestNoSource() => new();

    // --- projector deliveries -----------------------------------------------

    public static EventPage IncreasedEventPage() =>
        new() { Event = AnyEmpty("test.counter.Increased") };

    public static EventBook DeliveryBook(string domain, int n)
    {
        var book = new EventBook { Cover = new Cover { Domain = domain } };
        for (var i = 0; i < n; i++)
        {
            book.Pages.Add(IncreasedEventPage());
        }
        return book;
    }

    public static EventBook DeliveryNoCover()
    {
        var book = DeliveryBook("counter", 1);
        book.Cover = null;
        return book;
    }

    // --- process-manager triggers -------------------------------------------

    public static ProcessManagerHandleRequest PmTrigger(
        string domain,
        IReadOnlyList<string> fqs,
        EventBook? state,
        IReadOnlyDictionary<string, uint>? dest
    )
    {
        var trigger = new EventBook { Cover = new Cover { Domain = domain } };
        foreach (var fq in fqs)
        {
            trigger.Pages.Add(new EventPage { Event = AnyEmpty(fq) });
        }
        var req = new ProcessManagerHandleRequest { Trigger = trigger };
        if (state != null)
        {
            req.ProcessState = state;
        }
        if (dest != null)
        {
            foreach (var kv in dest)
            {
                req.DestinationSequences.Add(kv.Key, kv.Value);
            }
        }
        return req;
    }

    public static EventBook PmStateOf(int n)
    {
        var book = new EventBook();
        for (var i = 0; i < n; i++)
        {
            book.Pages.Add(IncreasedEventPage());
        }
        return book;
    }

    public static ProcessManagerHandleRequest PmRejection(string fqCommand)
    {
        var notification = RejectionNotificationFor(fqCommand, "inventory");
        return new ProcessManagerHandleRequest
        {
            Trigger = new EventBook
            {
                Cover = new Cover { Domain = "counter" },
                Pages = { new EventPage { Event = Pack.Wrap(notification) } },
            },
        };
    }

    public static ProcessManagerHandleRequest PmNoTrigger() => new();

    public static ProcessManagerHandleRequest PmEmptyTrigger() =>
        new() { Trigger = new EventBook() };
}
