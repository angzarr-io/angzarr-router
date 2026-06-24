package ffirouter

import (
	"google.golang.org/protobuf/types/known/anypb"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

// SagaEventThunk translates one source event into commands and/or injected
// fact events, stamping emitted commands from the supplied Destinations.
// sourceCover is the source book's cover, so the saga can route emitted
// commands by the trigger's identity (root, ext). Binding/generated thunks
// unmarshal to the typed event and call the typed business method.
type SagaEventThunk func(event *anypb.Any, dests *Destinations, sourceCover *pb.Cover) (commands []*pb.CommandBook, events []*pb.EventBook, err error)

// SagaRejectionThunk compensates a rejected command, returning fact events to
// inject. Multiple thunks for one command run in registration order (C-0042).
type SagaRejectionThunk func(n *pb.Notification, rejection *pb.RejectionNotification) ([]*pb.EventBook, error)

// SagaDispatch is one saga component's registration: its name, the input
// domain it consumes, the domains it issues commands to, its event handlers,
// and ordered rejection compensators. A saga is stateless — no rebuilder, no
// state. The shape mirrors the core's so generated wiring (unit 6) targets it
// with minimal emitter changes.
type SagaDispatch struct {
	name        string
	inputDomain string
	targets     []string
	events      map[string]SagaEventThunk
	rejections  map[string][]SagaRejectionThunk
}

// NewSagaDispatch starts a saga registration translating inputDomain events
// into commands for targetDomains.
func NewSagaDispatch(name, inputDomain string, targetDomains ...string) *SagaDispatch {
	return &SagaDispatch{
		name:        name,
		inputDomain: inputDomain,
		targets:     targetDomains,
		events:      make(map[string]SagaEventThunk),
		rejections:  make(map[string][]SagaRejectionThunk),
	}
}

// OnEvent registers the translation thunk for a fully-qualified event type.
func (d *SagaDispatch) OnEvent(fullName string, thunk SagaEventThunk) *SagaDispatch {
	d.events[fullName] = thunk
	return d
}

// OnRejected appends a compensator for one fully-qualified command type;
// repeated calls register an ordered fan-out (C-0042).
func (d *SagaDispatch) OnRejected(fqCommand string, thunk SagaRejectionThunk) *SagaDispatch {
	d.rejections[fqCommand] = append(d.rejections[fqCommand], thunk)
	return d
}

// Destinations provides the coordinator-supplied next-sequences for command
// stamping. Sagas and process managers are translators — they stamp emitted
// commands, they do not rebuild destination state to make decisions.
type Destinations struct {
	sequences map[string]uint32
}

// NewDestinations wraps a domain→next-sequence map (nil becomes empty).
func NewDestinations(sequences map[string]uint32) *Destinations {
	if sequences == nil {
		sequences = map[string]uint32{}
	}
	return &Destinations{sequences: sequences}
}

// SequenceFor returns the next sequence for a domain, and whether one exists.
func (d *Destinations) SequenceFor(domain string) (uint32, bool) {
	seq, ok := d.sequences[domain]
	return seq, ok
}

// Has reports whether a sequence exists for the domain.
func (d *Destinations) Has(domain string) bool {
	_, ok := d.sequences[domain]
	return ok
}

// Domains returns every domain carrying a sequence (unordered).
func (d *Destinations) Domains() []string {
	domains := make([]string, 0, len(d.sequences))
	for domain := range d.sequences {
		domains = append(domains, domain)
	}
	return domains
}

// StampCommand stamps every page of cmd with the next sequence for domain.
// A domain with no supplied sequence is the coded MISSING_DESTINATION_SEQUENCE
// (check output_domains config).
func (d *Destinations) StampCommand(cmd *pb.CommandBook, domain string) error {
	seq, ok := d.sequences[domain]
	if !ok {
		return &CodedError{
			Code:    "MISSING_DESTINATION_SEQUENCE",
			Message: "no sequence for destination domain",
			Grpc:    GrpcInvalidArgument,
			Extras:  map[string]string{"domain": domain},
		}
	}
	for _, page := range cmd.Pages {
		page.Header = &pb.PageHeader{SequenceType: &pb.PageHeader_Sequence{Sequence: seq}}
	}
	return nil
}
