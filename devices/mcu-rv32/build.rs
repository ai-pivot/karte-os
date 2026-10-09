fn main() {
    // 设备身份是编译期参数：env 变化必须触发重编
    println!("cargo:rerun-if-env-changed=KARTE_DEVICE_ID");
    println!("cargo:rerun-if-env-changed=KARTE_ROLE");
}
