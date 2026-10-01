package ffirouter

import (
	"google.golang.org/protobuf/types/known/anypb"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

// SagaEventThunk translates one source event into commands and/or injected
// fact events. dests are the saga's declared output domains; emitted commands
// are returned unstamped and the router stamps them deferred from the
// triggering event. sourceCover is the source book's cover, so the saga can
// route emitted commands by the trigger's identity (root, ext).
// Binding/generated thunks unmarshal to the typed event and call the typed
// business method.
type SagaEventThunk func(event *anypb.Any, dests *Destinations, sourceCover *pb.Cover) (commands []*pb.CommandBook, events []*pb.EventBook, err error)

// SagaRejectionThunk is the shape of a saga compensator. Sagas receive no
// rejections, so the router never invokes one.
type SagaRejectionThunk func(n *pb.Notification, rejection *pb.RejectionNotification) ([]*pb.EventBook, error)

// SagaDispatch is one saga component's registration: its name, the input
// domain it consumes, the domains it issues commands to, and its event
// handlers. A saga is stateless — no rebuilder, no state — and receives no
// rejections.
type SagaDispatch struct {
	name        string
	inputDomain string
	targets     []string
	events      map[string]SagaEventThunk
}

// NewSagaDispatch starts a saga registration translating inputDomain events
// into commands for targetDomains.
func NewSagaDispatch(name, inputDomain string, targetDomains ...string) *SagaDispatch {
	return &SagaDispatch{
		name:        name,
		inputDomain: inputDomain,
		targets:     targetDomains,
		events:      make(map[string]SagaEventThunk),
	}
}

// OnEvent registers the translation thunk for a fully-qualified event type.
func (d *SagaDispatch) OnEvent(fullName string, thunk SagaEventThunk) *SagaDispatch {
	d.events[fullName] = thunk
	return d
}

// OnRejected accepts a saga compensator and registers nothing: sagas receive
// no rejections (a rejection Notification in a saga's source emits nothing),
// so the thunk never runs. Compensation belongs to the aggregate or process
// manager that issued the command.
//
// Deprecated: sagas receive no rejections; declare compensates on the issuing
// aggregate or process manager instead.
func (d *SagaDispatch) OnRejected(string, SagaRejectionThunk) *SagaDispatch {
	return d
}

// Destinations are a translator's declared output domains: the domains a saga
// or process manager may issue commands to. Commands are returned unstamped;
// the router stamps them deferred.
type Destinations struct {
	domains []string
}

// NewDestinations declares the output domains, in order.
func NewDestinations(domains ...string) *Destinations {
	return &Destinations{domains: append([]string(nil), domains...)}
}

// Has reports whether domain is a declared output domain.
func (d *Destinations) Has(domain string) bool {
	for _, declared := range d.domains {
		if declared == domain {
			return true
		}
	}
	return false
}

// Domains returns the declared output domains in declaration order.
func (d *Destinations) Domains() []string {
	return append([]string(nil), d.domains...)
}
