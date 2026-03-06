package pyroscope

import (
	"bytes"
	"context"
	"fmt"
	"net/http"
	"testing"
	"time"

	pprofprofile "github.com/google/pprof/profile"
	"go.opentelemetry.io/collector/component"
	"go.opentelemetry.io/collector/component/componenttest"
	"go.opentelemetry.io/collector/config/confighttp"
	"go.opentelemetry.io/collector/config/confignet"
	"go.opentelemetry.io/collector/consumer/consumertest"
	"go.opentelemetry.io/collector/receiver/receivertest"
)

func TestIngestPprof(t *testing.T) {
	sink := new(consumertest.ProfilesSink)
	cfg := &Config{
		HTTP: confighttp.ServerConfig{
			NetAddr: confignet.AddrConfig{
				Endpoint:  "localhost:19532",
				Transport: confignet.TransportTypeTCP,
			},
		},
		Timeout: 5 * time.Second,
	}

	set := receivertest.NewNopSettings(component.MustNewType("pyroscope"))

	recv, err := newPyroscopeReceiver(cfg, sink, set)
	if err != nil {
		t.Fatalf("failed to create receiver: %v", err)
	}

	ctx := context.Background()
	if err := recv.Start(ctx, componenttest.NewNopHost()); err != nil {
		t.Fatalf("failed to start receiver: %v", err)
	}

	defer func() { _ = recv.Shutdown(ctx) }()

	// Build a minimal pprof profile.
	prof := &pprofprofile.Profile{
		SampleType: []*pprofprofile.ValueType{
			{Type: "cpu", Unit: "nanoseconds"},
		},
		PeriodType: &pprofprofile.ValueType{Type: "cpu", Unit: "nanoseconds"},
		Period:     10000000, // 10ms
		Sample: []*pprofprofile.Sample{
			{
				Location: []*pprofprofile.Location{
					{
						ID:      1,
						Address: 0x1000,
						Line: []pprofprofile.Line{
							{Function: &pprofprofile.Function{ID: 1, Name: "main.handler", Filename: "main.go", StartLine: 10}},
						},
					},
					{
						ID:      2,
						Address: 0x2000,
						Line: []pprofprofile.Line{
							{Function: &pprofprofile.Function{ID: 2, Name: "net/http.serve", Filename: "server.go", StartLine: 100}},
						},
					},
				},
				Value: []int64{42000000}, // 42ms of CPU
			},
		},
		Location: []*pprofprofile.Location{
			{ID: 1, Address: 0x1000, Line: []pprofprofile.Line{{Function: &pprofprofile.Function{ID: 1, Name: "main.handler", Filename: "main.go", StartLine: 10}}}},
			{ID: 2, Address: 0x2000, Line: []pprofprofile.Line{{Function: &pprofprofile.Function{ID: 2, Name: "net/http.serve", Filename: "server.go", StartLine: 100}}}},
		},
		Function: []*pprofprofile.Function{
			{ID: 1, Name: "main.handler", Filename: "main.go", StartLine: 10},
			{ID: 2, Name: "net/http.serve", Filename: "server.go", StartLine: 100},
		},
		TimeNanos:     1700000000000000000,
		DurationNanos: 10000000000,
	}

	var buf bytes.Buffer
	if err := prof.Write(&buf); err != nil {
		t.Fatalf("failed to write pprof: %v", err)
	}

	url := fmt.Sprintf("http://localhost:19532/ingest?name=my-service{env=staging}&from=1700000000&until=1700000010")

	resp, err := http.Post(url, "application/octet-stream", &buf)
	if err != nil {
		t.Fatalf("failed to POST: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body := make([]byte, 1024)
		n, _ := resp.Body.Read(body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body[:n]))
	}

	if sink.ProfileCount() == 0 {
		t.Fatal("expected at least one profile, got none")
	}

	profiles := sink.AllProfiles()[0]
	rp := profiles.ResourceProfiles().At(0)

	serviceName, ok := rp.Resource().Attributes().Get("service.name")
	if !ok || serviceName.Str() != "my-service" {
		t.Errorf("expected service.name=my-service, got %v", serviceName)
	}

	env, ok := rp.Resource().Attributes().Get("env")
	if !ok || env.Str() != "staging" {
		t.Errorf("expected env=staging, got %v", env)
	}

	t.Logf("Received %d profile resource(s)", profiles.ResourceProfiles().Len())
}

func TestParseIngestParams(t *testing.T) {
	tests := []struct {
		name    string
		query   string
		want    ingestParams
		wantErr bool
	}{
		{
			name:  "simple name",
			query: "name=my-app&from=1700000000&until=1700000010",
			want: ingestParams{
				name:   "my-app",
				labels: map[string]string{},
				start:  1700000000,
				end:    1700000010,
			},
		},
		{
			name:  "name with labels",
			query: "name=my-app{env=prod,region=us-west-1}&from=100",
			want: ingestParams{
				name:   "my-app",
				labels: map[string]string{"env": "prod", "region": "us-west-1"},
				start:  100,
			},
		},
		{
			name:    "missing name",
			query:   "from=100",
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			vals, err := http.NewRequest("GET", "/?"+tt.query, nil)
			if err != nil {
				t.Fatal(err)
			}

			got, err := parseIngestParams(vals.URL.Query())
			if tt.wantErr {
				if err == nil {
					t.Fatal("expected error")
				}

				return
			}

			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}

			if got.name != tt.want.name {
				t.Errorf("name: got %q, want %q", got.name, tt.want.name)
			}

			if got.start != tt.want.start {
				t.Errorf("start: got %d, want %d", got.start, tt.want.start)
			}

			if got.end != tt.want.end {
				t.Errorf("end: got %d, want %d", got.end, tt.want.end)
			}

			for k, v := range tt.want.labels {
				if got.labels[k] != v {
					t.Errorf("label %s: got %q, want %q", k, got.labels[k], v)
				}
			}
		})
	}
}

func TestNsFromSec(t *testing.T) {
	tests := []struct {
		input uint64
		want  uint64
	}{
		{1700000000, 1700000000000000000},          // seconds
		{1700000000000, 1700000000000000000},       // milliseconds
		{1700000000000000, 1700000000000000000},    // microseconds
		{1700000000000000000, 1700000000000000000}, // already ns
	}
	for _, tt := range tests {
		got := nsFromSec(tt.input)
		if got != tt.want {
			t.Errorf("nsFromSec(%d) = %d, want %d", tt.input, got, tt.want)
		}
	}
}
