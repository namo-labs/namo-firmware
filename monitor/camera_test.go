package main

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

// camServer는 플레이리스트를 흉내 냅니다. lastMod와 serverNow를 따로
// 두어, 스트림 서버의 시계가 이 서비스와 어긋난 상황을 만들 수 있습니다.
func camServer(t *testing.T, lastMod, serverNow time.Time, code int) *httptest.Server {
	t.Helper()
	s := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !lastMod.IsZero() {
			w.Header().Set("Last-Modified", lastMod.UTC().Format(http.TimeFormat))
		}
		if !serverNow.IsZero() {
			w.Header().Set("Date", serverNow.UTC().Format(http.TimeFormat))
		}
		w.WriteHeader(code)
	}))
	t.Cleanup(s.Close)
	return s
}

func probeOnce(t *testing.T, url string, stall time.Duration) cameraStatus {
	t.Helper()
	c := newCameraProbe(url, stall)
	c.probe(context.Background())
	return c.get()
}

func TestCamera갱신중이면live(t *testing.T) {
	now := time.Now()
	s := camServer(t, now.Add(-2*time.Second), now, http.StatusOK)

	if got := probeOnce(t, s.URL, 25*time.Second); got.Status != camLive {
		t.Fatalf("status = %q, 기대 live (%s)", got.Status, got.Error)
	}
}

// 이 테스트가 이 파일의 이유입니다. 서버는 200을 주고 세그먼트도
// 받아지지만 프레임은 몇 시간 전에 멈춘 상태입니다.
func TestCamera얼어붙으면stalled(t *testing.T) {
	now := time.Now()
	s := camServer(t, now.Add(-6*time.Hour), now, http.StatusOK)

	got := probeOnce(t, s.URL, 25*time.Second)
	if got.Status != camStalled {
		t.Fatalf("status = %q, 기대 stalled", got.Status)
	}
	if got.AgeS == nil || *got.AgeS < 6*3600-5 {
		t.Fatalf("age_s = %v, 6시간쯤이어야 합니다", got.AgeS)
	}
}

// 이 서비스는 클러스터에서, 스트림 서버는 맥에서 돕니다. 두 시계가
// 어긋나도 판정은 흔들리면 안 됩니다. 같은 응답 안의 Date와
// Last-Modified만 빼기 때문입니다.
func TestCamera시계가어긋나도흔들리지않는다(t *testing.T) {
	skewed := time.Now().Add(3 * time.Hour)
	s := camServer(t, skewed.Add(-2*time.Second), skewed, http.StatusOK)

	if got := probeOnce(t, s.URL, 25*time.Second); got.Status != camLive {
		t.Fatalf("status = %q, 기대 live — 시계 차이에 흔들렸습니다", got.Status)
	}
}

func TestCamera서버가꺼지면down(t *testing.T) {
	s := camServer(t, time.Now(), time.Now(), http.StatusOK)
	url := s.URL
	s.Close()

	if got := probeOnce(t, url, 25*time.Second); got.Status != camDown {
		t.Fatalf("status = %q, 기대 down", got.Status)
	}
}

func TestCamera404면down(t *testing.T) {
	s := camServer(t, time.Time{}, time.Now(), http.StatusNotFound)

	if got := probeOnce(t, s.URL, 25*time.Second); got.Status != camDown {
		t.Fatalf("status = %q, 기대 down", got.Status)
	}
}

// 신선도를 모르면 살아 있다고 하지 않습니다. "받아지니까 괜찮다"가
// 바로 지금 고치려는 착각입니다.
func TestCameraLastModified가없으면unknown(t *testing.T) {
	s := camServer(t, time.Time{}, time.Now(), http.StatusOK)

	if got := probeOnce(t, s.URL, 25*time.Second); got.Status != camUnknown {
		t.Fatalf("status = %q, 기대 unknown", got.Status)
	}
}

// 서버가 Date를 주지 않으면 이쪽 시계로 재는 수밖에 없습니다.
func TestCameraDate가없어도판정한다(t *testing.T) {
	s := camServer(t, time.Now().Add(-6*time.Hour), time.Time{}, http.StatusOK)

	if got := probeOnce(t, s.URL, 25*time.Second); got.Status != camStalled {
		t.Fatalf("status = %q, 기대 stalled", got.Status)
	}
}

func TestCamera첫관측은전이가아니다(t *testing.T) {
	s := camServer(t, time.Now(), time.Now(), http.StatusOK)
	c := newCameraProbe(s.URL, 25*time.Second)

	from, to := c.probe(context.Background())
	if from != camUnknown || to != camLive {
		t.Fatalf("첫 관측 = %q → %q, 기대 unknown → live", from, to)
	}
	// 같은 상태가 이어지면 전이가 아닙니다.
	if from, to := c.probe(context.Background()); from != "" || to != "" {
		t.Fatalf("같은 상태인데 전이로 봤습니다: %q → %q", from, to)
	}
}

// ── 알림 ───────────────────────────────────────────────────────────

func TestCamera경보는유예뒤에한번만(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.checkCamera(camStalled, now)
	if len(f.sent) != 0 {
		t.Fatalf("유예 안에 보냈습니다: %v", f.sent)
	}

	r.checkCamera(camStalled, now.Add(4*time.Minute))
	if len(f.sent) != 0 {
		t.Fatalf("유예 안에 보냈습니다: %v", f.sent)
	}

	r.checkCamera(camStalled, now.Add(6*time.Minute))
	r.checkCamera(camStalled, now.Add(7*time.Minute))
	if len(f.sent) != 1 {
		t.Fatalf("보낸 횟수 = %d, 기대 1: %v", len(f.sent), f.sent)
	}
	if !contains(f.sent[0], "카메라가 멈췄습니다") {
		t.Fatalf("내용이 다릅니다: %s", f.sent[0])
	}
}

func TestCamera돌아오면해제(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.checkCamera(camStalled, now)
	r.checkCamera(camStalled, now.Add(6*time.Minute))
	r.checkCamera(camLive, now.Add(7*time.Minute))

	if len(f.sent) != 2 {
		t.Fatalf("보낸 횟수 = %d, 기대 2: %v", len(f.sent), f.sent)
	}
	if !contains(f.sent[1], "카메라가 돌아왔습니다") {
		t.Fatalf("복구 알림이 아닙니다: %s", f.sent[1])
	}
}

// 아직 확인 전인 것을 고장으로 알리면, 서비스를 띄울 때마다 알림이
// 한 통씩 갑니다.
func TestCameraUnknown으로는알리지않는다(t *testing.T) {
	r, _, f := newTestRules(50)
	now := time.Now()

	r.checkCamera(camUnknown, now)
	r.checkCamera(camUnknown, now.Add(30*time.Minute))

	if len(f.sent) != 0 {
		t.Fatalf("unknown으로 알렸습니다: %v", f.sent)
	}
}
