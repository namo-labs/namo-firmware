package main

import (
	"context"
	"log"
	"net/http"
	"sync"
	"time"
)

// 카메라 생존 판정.
//
// 이 웹캠은 프레임 공급을 멈추면서도 ffmpeg 프로세스와 HTTP 서버는 살아
// 있는 상태로 얼어붙습니다. 그러면 플레이리스트는 200을 그대로 주고
// 마지막 세그먼트도 받아지므로, 재생기는 몇 시간 전 화면을 틀면서
// "LIVE"를 띄웁니다. 실제로 6시간을 그렇게 보냈습니다.
//
// 그래서 **받아지는가가 아니라 방금 쓰였는가**를 봅니다. 플레이리스트의
// Last-Modified가 답입니다. 세그먼트가 1초마다 갱신되므로, 이 값이 멈춰
// 있으면 카메라도 멈춘 것입니다.
const (
	camLive    = "live"    // 갱신 중
	camStalled = "stalled" // 서버는 응답하지만 프레임이 멈춤
	camDown    = "down"    // 서버에 닿지 못함 (cam.sh 자체가 꺼짐)
	camUnknown = "unknown" // 아직 확인 전
)

type cameraStatus struct {
	Status string `json:"status"`
	// 마지막 프레임이 몇 초 전에 쓰였는지. 모르면 생략합니다.
	AgeS *int `json:"age_s,omitempty"`
	// 닿지 못한 이유. 사람이 읽고 어디를 볼지 정할 수 있어야 합니다.
	Error string `json:"error,omitempty"`
	// 마지막으로 확인한 지 몇 초 지났는지.
	CheckedAgoS int `json:"checked_ago_s"`
}

type cameraProbe struct {
	url    string
	stall  time.Duration
	client *http.Client

	mu      sync.RWMutex
	status  string
	ageS    int
	errMsg  string
	checked time.Time
}

func newCameraProbe(url string, stall time.Duration) *cameraProbe {
	return &cameraProbe{
		url:   url,
		stall: stall,
		// 맥이 잠겨 있으면 응답이 늦을 수 있지만, 오래 붙잡고 있을
		// 이유는 없습니다. 늦으면 그것도 정상이 아닙니다.
		client: &http.Client{Timeout: 5 * time.Second},
		status: camUnknown,
		ageS:   -1,
	}
}

func (c *cameraProbe) get() cameraStatus {
	c.mu.RLock()
	defer c.mu.RUnlock()

	s := cameraStatus{Status: c.status, Error: c.errMsg}
	if c.ageS >= 0 {
		age := c.ageS
		s.AgeS = &age
	}
	if !c.checked.IsZero() {
		s.CheckedAgoS = int(time.Since(c.checked).Seconds())
	}
	return s
}

// probe는 한 번 확인하고, 상태가 바뀌었으면 이전 상태를 함께 돌려줍니다.
func (c *cameraProbe) probe(ctx context.Context) (from, to string) {
	status, age, errMsg := c.check(ctx)

	c.mu.Lock()
	defer c.mu.Unlock()
	from = c.status
	c.status, c.ageS, c.errMsg, c.checked = status, age, errMsg, time.Now()
	if from == status {
		return "", ""
	}
	return from, status
}

func (c *cameraProbe) check(ctx context.Context) (status string, ageS int, errMsg string) {
	// 본문은 필요 없습니다. 헤더만으로 판정합니다.
	req, err := http.NewRequestWithContext(ctx, http.MethodHead, c.url, nil)
	if err != nil {
		return camDown, -1, err.Error()
	}
	resp, err := c.client.Do(req)
	if err != nil {
		return camDown, -1, "스트림 서버에 닿지 못했습니다"
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return camDown, -1, "스트림 서버가 " + resp.Status + "를 돌려줬습니다"
	}

	lm, err := http.ParseTime(resp.Header.Get("Last-Modified"))
	if err != nil {
		// 헤더가 없으면 신선도를 알 수 없습니다. 받아진다는 것만으로
		// 살아 있다고 하면 지금 고치려는 바로 그 착각을 되풀이하게
		// 되므로, 모른다고 말합니다.
		return camUnknown, -1, "Last-Modified를 읽지 못했습니다"
	}

	// 나이는 **스트림 서버의 시계로** 잽니다. 이 서비스는 클러스터
	// 안에서 돌고 스트림 서버는 맥에서 돌아, 두 시계가 조금 어긋나도
	// 같은 쪽 시계끼리 빼면 오차가 사라집니다.
	now, err := http.ParseTime(resp.Header.Get("Date"))
	if err != nil {
		now = time.Now()
	}

	age := now.Sub(lm)
	if age < 0 {
		age = 0
	}
	if age > c.stall {
		return camStalled, int(age.Seconds()), "세그먼트가 갱신되지 않고 있습니다"
	}
	return camLive, int(age.Seconds()), ""
}

// watchCamera는 카메라를 주기적으로 확인하며 변화를 사건으로 남기고
// 알립니다.
func watchCamera(c *cameraProbe, events *EventStore, alerts *alertRules, every time.Duration) {
	t := time.NewTicker(every)
	defer t.Stop()

	for {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		from, to := c.probe(ctx)
		cancel()

		if to != "" {
			log.Printf("카메라: %s → %s", from, to)
			// 첫 관측은 전이가 아닙니다. 서비스를 다시 띄울 때마다
			// 사건이 하나씩 쌓이면 기록이 지저분해집니다.
			if from != camUnknown {
				events.Add(Event{
					TS: time.Now().Unix(), Kind: KindCamera,
					From: from, To: to,
				})
			}
		}
		alerts.checkCamera(c.get().Status, time.Now())

		<-t.C
	}
}
