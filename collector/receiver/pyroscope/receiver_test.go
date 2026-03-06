package pyroscope

import (
	"bytes"
	"compress/gzip"
	"context"
	"fmt"
	"io"
	"net/http"
	"sync"
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

func TestIngestGzipWithoutContentEncoding(t *testing.T) {
	sink := new(consumertest.ProfilesSink)
	cfg := &Config{
		HTTP: confighttp.ServerConfig{
			NetAddr: confignet.AddrConfig{
				Endpoint:  "localhost:19533",
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

	prof := minimalProfile()

	var raw bytes.Buffer
	if err := prof.Write(&raw); err != nil {
		t.Fatalf("failed to write pprof: %v", err)
	}

	// Gzip the body but do NOT set Content-Encoding header.
	var compressed bytes.Buffer

	gz := gzip.NewWriter(&compressed)
	if _, err := gz.Write(raw.Bytes()); err != nil {
		t.Fatalf("gzip write: %v", err)
	}

	if err := gz.Close(); err != nil {
		t.Fatalf("gzip close: %v", err)
	}

	resp, err := http.Post(
		"http://localhost:19533/ingest?name=gzip-test",
		"application/octet-stream",
		&compressed,
	)
	if err != nil {
		t.Fatalf("POST failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, body)
	}

	if sink.ProfileCount() == 0 {
		t.Fatal("expected at least one profile")
	}

	serviceName, _ := sink.AllProfiles()[0].ResourceProfiles().At(0).Resource().Attributes().Get("service.name")
	if serviceName.Str() != "gzip-test" {
		t.Errorf("service.name: got %q, want %q", serviceName.Str(), "gzip-test")
	}
}

func TestIngestInvalidPprof(t *testing.T) {
	sink := new(consumertest.ProfilesSink)
	cfg := &Config{
		HTTP: confighttp.ServerConfig{
			NetAddr: confignet.AddrConfig{
				Endpoint:  "localhost:19534",
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

	// Send garbage data — should get 400, not panic.
	resp, err := http.Post(
		"http://localhost:19534/ingest?name=bad-data",
		"application/octet-stream",
		bytes.NewReader([]byte("this is not pprof data")),
	)
	if err != nil {
		t.Fatalf("POST failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("expected 400 for invalid pprof, got %d", resp.StatusCode)
	}

	if sink.ProfileCount() != 0 {
		t.Fatal("expected no profiles for invalid data")
	}
}

func TestIngestConcurrent(t *testing.T) {
	sink := new(consumertest.ProfilesSink)
	cfg := &Config{
		HTTP: confighttp.ServerConfig{
			NetAddr: confignet.AddrConfig{
				Endpoint:  "localhost:19535",
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

	prof := minimalProfile()

	var raw bytes.Buffer
	if err := prof.Write(&raw); err != nil {
		t.Fatalf("failed to write pprof: %v", err)
	}

	profileBytes := raw.Bytes()

	const numRequests = 20

	var wg sync.WaitGroup

	errs := make(chan error, numRequests)

	for i := range numRequests {
		wg.Add(1)

		go func(idx int) {
			defer wg.Done()

			resp, err := http.Post(
				fmt.Sprintf("http://localhost:19535/ingest?name=concurrent-svc-%d", idx),
				"application/octet-stream",
				bytes.NewReader(profileBytes),
			)
			if err != nil {
				errs <- fmt.Errorf("request %d: %w", idx, err)
				return
			}
			defer resp.Body.Close()

			if resp.StatusCode != http.StatusOK {
				body, _ := io.ReadAll(resp.Body)
				errs <- fmt.Errorf("request %d: status %d: %s", idx, resp.StatusCode, body)
			}
		}(i)
	}

	wg.Wait()
	close(errs)

	for err := range errs {
		t.Error(err)
	}

	if sink.ProfileCount() != numRequests {
		t.Errorf("expected %d profiles, got %d", numRequests, sink.ProfileCount())
	}
}

func TestIngestLargeProfile(t *testing.T) {
	sink := new(consumertest.ProfilesSink)
	cfg := &Config{
		HTTP: confighttp.ServerConfig{
			NetAddr: confignet.AddrConfig{
				Endpoint:  "localhost:19536",
				Transport: confignet.TransportTypeTCP,
			},
		},
		Timeout: 10 * time.Second,
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

	// Build a large profile with many locations and samples (deep stacks).
	const (
		numFunctions = 500
		numSamples   = 200
	)

	functions := make([]*pprofprofile.Function, numFunctions)
	locations := make([]*pprofprofile.Location, numFunctions)

	for i := range numFunctions {
		functions[i] = &pprofprofile.Function{
			ID:        uint64(i + 1),
			Name:      fmt.Sprintf("pkg/module%d.Function%d", i/10, i),
			Filename:  fmt.Sprintf("pkg/module%d/file.go", i/10),
			StartLine: int64(i*10 + 1),
		}
		locations[i] = &pprofprofile.Location{
			ID:      uint64(i + 1),
			Address: uint64(0x1000 + i*0x100),
			Line:    []pprofprofile.Line{{Function: functions[i], Line: int64(i*10 + 5)}},
		}
	}

	samples := make([]*pprofprofile.Sample, numSamples)
	for i := range numSamples {
		// Each sample gets a stack of 50 frames (subset of locations).
		stackDepth := 50

		stack := make([]*pprofprofile.Location, stackDepth)
		for j := range stackDepth {
			stack[j] = locations[(i*7+j)%numFunctions]
		}

		samples[i] = &pprofprofile.Sample{
			Location: stack,
			Value:    []int64{int64((i + 1) * 1000000)},
		}
	}

	prof := &pprofprofile.Profile{
		SampleType: []*pprofprofile.ValueType{{Type: "cpu", Unit: "nanoseconds"}},
		PeriodType: &pprofprofile.ValueType{Type: "cpu", Unit: "nanoseconds"},
		Period:     10000000,
		Sample:     samples,
		Location:   locations,
		Function:   functions,
		TimeNanos:  1700000000000000000,
	}

	var buf bytes.Buffer
	if err := prof.Write(&buf); err != nil {
		t.Fatalf("failed to write large pprof: %v", err)
	}

	t.Logf("Large profile size: %d bytes (%d functions, %d samples)", buf.Len(), numFunctions, numSamples)

	resp, err := http.Post(
		"http://localhost:19536/ingest?name=large-profile-test",
		"application/octet-stream",
		&buf,
	)
	if err != nil {
		t.Fatalf("POST failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, body)
	}

	if sink.ProfileCount() == 0 {
		t.Fatal("expected at least one profile for large input")
	}
}

// minimalProfile returns a small valid pprof profile for test reuse.
func minimalProfile() *pprofprofile.Profile {
	return &pprofprofile.Profile{
		SampleType: []*pprofprofile.ValueType{{Type: "cpu", Unit: "nanoseconds"}},
		PeriodType: &pprofprofile.ValueType{Type: "cpu", Unit: "nanoseconds"},
		Period:     10000000,
		Sample: []*pprofprofile.Sample{
			{
				Location: []*pprofprofile.Location{
					{ID: 1, Address: 0x1000, Line: []pprofprofile.Line{
						{Function: &pprofprofile.Function{ID: 1, Name: "main.handler", Filename: "main.go", StartLine: 10}},
					}},
				},
				Value: []int64{42000000},
			},
		},
		Location: []*pprofprofile.Location{
			{ID: 1, Address: 0x1000, Line: []pprofprofile.Line{
				{Function: &pprofprofile.Function{ID: 1, Name: "main.handler", Filename: "main.go", StartLine: 10}},
			}},
		},
		Function: []*pprofprofile.Function{
			{ID: 1, Name: "main.handler", Filename: "main.go", StartLine: 10},
		},
		TimeNanos:     1700000000000000000,
		DurationNanos: 10000000000,
	}
}

func TestIngestFoldedStacks(t *testing.T) {
	sink := new(consumertest.ProfilesSink)
	cfg := &Config{
		HTTP: confighttp.ServerConfig{
			NetAddr: confignet.AddrConfig{
				Endpoint:  "localhost:19537",
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

	folded := []byte(`main;init;setup 100
main;process;compute 250
main;process;io_wait 75
main;cleanup 30
`)

	// Test with explicit format=folded.
	resp, err := http.Post(
		"http://localhost:19537/ingest?name=java-service{env=prod}&format=folded&from=1700000000&until=1700000010",
		"text/plain",
		bytes.NewReader(folded),
	)
	if err != nil {
		t.Fatalf("POST failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, body)
	}

	if sink.ProfileCount() == 0 {
		t.Fatal("expected at least one profile")
	}

	profiles := sink.AllProfiles()[0]
	rp := profiles.ResourceProfiles().At(0)

	serviceName, ok := rp.Resource().Attributes().Get("service.name")
	if !ok || serviceName.Str() != "java-service" {
		t.Errorf("expected service.name=java-service, got %v", serviceName)
	}

	env, ok := rp.Resource().Attributes().Get("env")
	if !ok || env.Str() != "prod" {
		t.Errorf("expected env=prod, got %v", env)
	}
}

func TestIngestFoldedStacksAutoDetect(t *testing.T) {
	sink := new(consumertest.ProfilesSink)
	cfg := &Config{
		HTTP: confighttp.ServerConfig{
			NetAddr: confignet.AddrConfig{
				Endpoint:  "localhost:19538",
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

	// Send folded stacks WITHOUT format param — should auto-detect via pprof fallback.
	folded := []byte(`com.example.App.main;com.example.App.process;com.example.App.compute 500
com.example.App.main;com.example.App.handleRequest 200
`)

	resp, err := http.Post(
		"http://localhost:19538/ingest?name=java-auto-detect",
		"text/plain",
		bytes.NewReader(folded),
	)
	if err != nil {
		t.Fatalf("POST failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, body)
	}

	if sink.ProfileCount() == 0 {
		t.Fatal("expected at least one profile from auto-detected folded stacks")
	}

	serviceName, _ := sink.AllProfiles()[0].ResourceProfiles().At(0).Resource().Attributes().Get("service.name")
	if serviceName.Str() != "java-auto-detect" {
		t.Errorf("service.name: got %q, want %q", serviceName.Str(), "java-auto-detect")
	}
}

func TestParseFoldedStacks(t *testing.T) {
	tests := []struct {
		name      string
		input     string
		wantErr   bool
		wantCount int // number of samples
	}{
		{
			name:      "basic",
			input:     "a;b;c 100\nd;e 50\n",
			wantCount: 2,
		},
		{
			name:      "single stack",
			input:     "main 42\n",
			wantCount: 1,
		},
		{
			name:      "with blank lines",
			input:     "\na;b 10\n\nc;d 20\n\n",
			wantCount: 2,
		},
		{
			name:    "empty input",
			input:   "",
			wantErr: true,
		},
		{
			name:    "only blank lines",
			input:   "\n\n\n",
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			prof, err := parseFoldedStacks([]byte(tt.input))
			if tt.wantErr {
				if err == nil {
					t.Fatal("expected error")
				}

				return
			}

			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}

			if len(prof.Sample) != tt.wantCount {
				t.Errorf("sample count: got %d, want %d", len(prof.Sample), tt.wantCount)
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
