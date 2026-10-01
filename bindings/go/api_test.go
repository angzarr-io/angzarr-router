package ffirouter

import (
	"errors"
	"strings"
	"testing"

	pb "github.com/angzarr-io/angzarr-router/bindings/go/gen/io/angzarr/v1"
)

func TestPack_UsesFrameworkBareSlashTypeURL(t *testing.T) {
	any, err := Pack(&pb.Notification{})
	if err != nil {
		t.Fatalf("Pack: %v", err)
	}
	if want := "/io.angzarr.v1.Notification"; any.TypeUrl != want {
		t.Errorf("TypeUrl = %q, want %q (bare-slash, not type.googleapis.com)", any.TypeUrl, want)
	}
}

func TestAnyDecodeError_IsInvalidArgumentWithTypeURL(t *testing.T) {
	err := AnyDecodeError("/io.angzarr.v1.Notification", errors.New("boom"))
	if err.Code != codeAnyDecodeFailed {
		t.Errorf("Code = %q, want %q", err.Code, codeAnyDecodeFailed)
	}
	if err.Grpc != GrpcInvalidArgument {
		t.Errorf("Grpc = %v, want InvalidArgument", err.Grpc)
	}
	if err.Extras["type_url"] != "/io.angzarr.v1.Notification" {
		t.Errorf("Extras[type_url] = %q, want the type URL", err.Extras["type_url"])
	}
}

func TestCheckAbiVersion_AcceptsTheExpectedVersion(t *testing.T) {
	if err := checkAbiVersion(ExpectedAbiVersion); err != nil {
		t.Fatalf("checkAbiVersion(%d) = %v, want nil", ExpectedAbiVersion, err)
	}
}

func TestCheckAbiVersion_RefusesADriftedLibraryNamingBothVersions(t *testing.T) {
	err := checkAbiVersion(ExpectedAbiVersion + 1)
	if err == nil {
		t.Fatal("checkAbiVersion accepted a mismatched ABI version")
	}
	msg := err.Error()
	if !strings.Contains(msg, "expects ABI version 3") || !strings.Contains(msg, "reports ABI version 4") {
		t.Errorf("error %q does not name expected (3) and actual (4) versions", msg)
	}
}

func TestDestinations_AreTheDeclaredOutputDomains(t *testing.T) {
	d := NewDestinations("inventory", "billing")
	if !d.Has("inventory") || !d.Has("billing") {
		t.Errorf("Has(declared) = false; domains %v", d.Domains())
	}
	if d.Has("shipping") {
		t.Error("Has(undeclared shipping) = true, want false")
	}
	got := d.Domains()
	if len(got) != 2 || got[0] != "inventory" || got[1] != "billing" {
		t.Errorf("Domains() = %v, want [inventory billing] in declaration order", got)
	}
}

func TestDestinations_EmptyDeclaresNothing(t *testing.T) {
	d := NewDestinations()
	if d.Has("") || len(d.Domains()) != 0 {
		t.Errorf("empty Destinations = %v, want none", d.Domains())
	}
}
