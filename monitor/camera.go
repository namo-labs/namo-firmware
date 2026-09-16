package main

import (
	"bufio"
	"context"
	"io"
	"log"
	"net/http"
	"strings"
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
// 그래서 **받아지는가가 아니라 새 프레임이 나왔는가**를 봅니다.
//
// 파일이 쓰인 시각만으로는 모자랍니다. ffmpeg은 종료할 때도 플레이리스트를
// 다시 쓰기 때문에, 감시 스크립트가 10초마다 재시작을 반복하는 동안 파일은
// 계속 새것처럼 보입니다. 프레임은 한 장도 안 나오는데 말입니다.
//
// 세그먼트 번호는 그렇지 않습니다. 한 장이 실제로 나와야 늘어납니다.
// 그래서 EXT-X-MEDIA-SEQUENCE가 언제 마지막으로 늘었는지를 함께 봅니다.
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

	// 마지막으로 본 세그먼트 번호와, 그것이 바뀐 시각.
	// 시각은 **스트림 서버의 시계로** 적습니다. 나이를 잴 때 같은 쪽
	// 시계끼리 빼야 클러스터와 맥의 시계 차이가 사라집니다.
	lastSeq   string
	lastSeqAt time.Time
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
	// 플레이리스트는 200바이트 남짓입니다. 세그먼트 번호를 읽어야
	// 하므로 본문까지 받습니다.
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.url, nil)
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

	// 나이는 **스트림 서버의 시계로** 잽니다. 이 서비스는 클러스터
	// 안에서 돌고 스트림 서버는 맥에서 돌아, 두 시계가 조금 어긋나도
	// 같은 쪽 시계끼리 빼면 오차가 사라집니다.
	now, err := http.ParseTime(resp.Header.Get("Date"))
	if err != nil {
		now = time.Now()
	}

	seq, seqErr := mediaSequence(resp.Body)
	lm, lmErr := http.ParseTime(resp.Header.Get("Last-Modified"))
	if seqErr != nil && lmErr != nil {
		// 둘 다 읽지 못하면 신선도를 알 수 없습니다. 받아진다는
		// 것만으로 살아 있다고 하면 지금 고치려는 바로 그 착각을
		// 되풀이하게 되므로, 모른다고 말합니다.
		return camUnknown, -1, "플레이리스트에서 신선도를 읽지 못했습니다"
	}

	age := time.Duration(-1)
	why := ""

	// 세그먼트 번호가 먼저입니다. 프레임이 실제로 나와야 늘어납니다.
	if seqErr == nil {
		c.mu.Lock()
		if seq != c.lastSeq {
			c.lastSeq, c.lastSeqAt = seq, now
		} else if c.lastSeqAt.IsZero() {
			c.lastSeqAt = now
		}
		since := now.Sub(c.lastSeqAt)
		c.mu.Unlock()

		age, why = since, "새 세그먼트가 나오지 않고 있습니다"
	}

	// 파일이 쓰인 시각도 봅니다. 서버가 통째로 얼어붙으면 이쪽이
	// 먼저 늙습니다. 둘 중 더 나쁜 쪽을 답으로 씁니다.
	if lmErr == nil {
		if lmAge := now.Sub(lm); lmAge > age {
			age, why = lmAge, "플레이리스트가 갱신되지 않고 있습니다"
		}
	}

	if age < 0 {
		age = 0
	}
	if age > c.stall {
		return camStalled, int(age.Seconds()), why
	}
	return camLive, int(age.Seconds()), ""
}

// mediaSequence는 플레이리스트에서 EXT-X-MEDIA-SEQUENCE 값을 읽습니다.
//
// 이 번호는 세그먼트가 하나 만들어질 때마다 늘어납니다. 파일이 다시
// 쓰이는 것과 달리, 프레임이 실제로 나오지 않으면 제자리입니다.
func mediaSequence(r io.Reader) (string, error) {
	const tag = "#EXT-X-MEDIA-SEQUENCE:"

	sc := bufio.NewScanner(io.LimitReader(r, 64*1024))
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if v, ok := strings.CutPrefix(line, tag); ok {
			return strings.TrimSpace(v), nil
		}
	}
	if err := sc.Err(); err != nil {
		return "", err
	}
	return "", errNoSequence
}

type noSequence struct{}

func (noSequence) Error() string { return "EXT-X-MEDIA-SEQUENCE가 없습니다" }

var errNoSequence = noSequence{}

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
