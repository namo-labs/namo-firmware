fn main() {
    embuild::espidf::sysenv::output();

    // pumprun의 구동 시간과 카운트다운을 빌드할 때 바꿀 수 있게 합니다.
    // 값이 바뀌면 다시 컴파일해야 하므로 cargo에 알려줍니다.
    println!("cargo:rerun-if-env-changed=PUMP_RUN_MS");
    println!("cargo:rerun-if-env-changed=PUMP_COUNTDOWN_S");
}
