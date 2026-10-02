//! 「组装 wpr 时用原图、不压缩」的**字节级**端到端断言。
//!
//! 用户问题：原始地图分辨率就这么低吗？组装 `.wpr` 的过程中请用原图，不要压缩。
//! 这条测试把链路两端钉死（中间每一段都是真实实现，不是复刻）：
//!
//! ```text
//! 假 8111 的 HTTP 响应体
//!   → wp8f_core::channel::Channel::read_map_img   （真实取图路径）
//!   → wpr::Record { map_img }                     （记录线程的打包路径）
//!   → Record::encode()  → .wpr 字节
//!   → ContainerHead::image() / Record::decode()   （回放取图的路径）
//! ```
//!
//! 断言：**回放里取到的底图字节与 8111 发出来的逐字节相同**，且
//! `meta.map.img_w/img_h` 等于"从这些字节解出来的真实像素尺寸"（不是另抄一份数字）。
//! 换句话说：记录端没有解码、没有重编码、没有缩放、没有二次压缩。

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use wp8f_core::channel::Channel;
use wp8f_logger::wpr::{self, CsvPool, MapMeta, Record, RecordMeta, RecordRow, NUM_COLS};

/// 假 8111：只服务**一次**请求，返回 `body` 作为 HTTP 响应体。
/// 返回（端口，服务线程）—— 服务线程交出它实际写出去的响应体长度，便于交叉核对。
fn serve_map_img_once(body: Vec<u8>) -> (u16, std::thread::JoinHandle<usize>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("绑定本地端口");
    let port = listener.local_addr().unwrap().port();
    let sent = body.clone();
    let handle = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("接受连接");
        let mut req = [0u8; 2048];
        let _ = sock.read(&mut req); // 请求内容不重要（真实实现也只发 GET）
        let mut resp = Vec::with_capacity(sent.len() + 128);
        resp.extend_from_slice(b"HTTP/1.1 200 OK\r\nServer: fake-8111\r\n");
        resp.extend_from_slice(b"Content-Type: image/jpeg\r\n");
        resp.extend_from_slice(format!("Content-Length: {}\r\n\r\n", sent.len()).as_bytes());
        resp.extend_from_slice(&sent);
        sock.write_all(&resp).expect("写出响应");
        sock.flush().ok();
        sent.len()
    });
    (port, handle)
}

/// 一张 64×48 的 JPEG（与 test-server 同款：`image` 编码的确定性渐变图）。
/// 用真实 JPEG 而不是随手几个字节：解码尺寸、像素往返都有意义。
fn test_jpeg() -> Vec<u8> {
    let (w, h) = (64u32, 48u32);
    let mut img = image::RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([(x * 4) as u8, (y * 5) as u8, ((x + y) * 2) as u8]);
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode(&img, w, h, image::ExtendedColorType::Rgb8)
        .expect("编码夹具 JPEG");
    out
}

#[test]
fn recorded_map_image_is_byte_for_byte_what_8111_served() {
    let jpeg = test_jpeg();
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "夹具必须是 JPEG（取图路径按 SOI 认响应体）");
    assert!(jpeg.len() > 256, "夹具不能是空壳：{} 字节", jpeg.len());

    let (port, srv) = serve_map_img_once(jpeg.clone());

    // ① 记录端从 8111 读到的字节：与服务器发出去的逐字节相同
    let mut chan = Channel::with_ip_and_ports(Ipv4Addr::LOCALHOST, vec![port]);
    let mut got = Vec::new();
    chan.read_map_img(&mut got).expect("从（假）8111 读 /map.img");
    let served_len = srv.join().expect("服务线程");
    assert_eq!(served_len, jpeg.len());
    assert_eq!(got, jpeg, "Channel::read_map_img 必须原样交出响应体（不解码/不缩放）");

    // ② 真实像素尺寸：从这些字节解出来 —— 记录端 meta 里的 img_w/img_h 就是它
    let rgb = image::load_from_memory(&got).expect("解 JPEG").to_rgb8();
    let (w, h) = rgb.dimensions();
    assert_eq!((w, h), (64, 48), "夹具的实际像素尺寸");

    // ③ 记录线程的打包路径：这一份字节进 Record.map_img（与 logger::thread::write_record 同形）
    let mut pool = CsvPool::new(0);
    let mut v = [0f64; NUM_COLS];
    v[wpr::col::ALTITUDE] = 3000.0;
    pool.push(&RecordRow { time_ns: 1_700_000_000_000_000_000, type_str: "f_16c".into(), v });
    let rec = Record {
        meta: RecordMeta {
            poll_hz: 30.0,
            map: MapMeta {
                width_m: 131_072.0,
                height_m: 131_072.0,
                min_x_m: -65_536.0,
                min_y_m: -65_536.0,
                img_w: w,
                img_h: h,
                img_mime: "image/jpeg".into(),
                ..Default::default()
            },
            aircraft: "f_16c".into(),
            ..Default::default()
        },
        groups: vec![pool.into_group()],
        map_img: got.clone(),
    };
    let bytes = rec.encode().expect("打包 .wpr");

    // ④ 回放侧取图：容器头（只解头 + meta，不碰 CSV）与完整解码都要给出**同一份原始字节**
    let head = wpr::ContainerHead::decode(&bytes).expect("解容器头");
    assert_eq!(head.img_len(), jpeg.len(), "头里的 img_len 就是原始字节数（不是解压后的大小）");
    assert_eq!(head.image(&bytes), &jpeg[..], ".wpr 里的底图必须与 8111 的响应体逐字节相同");
    let back = Record::decode(&bytes).expect("解完整记录");
    assert_eq!(back.map_img, jpeg, "解码往返后底图字节不变");
    assert_eq!((back.meta.map.img_w, back.meta.map.img_h), (w, h), "meta 尺寸 = 字节里的真实尺寸");

    // ⑤ 同一份字节解出来的像素也一致（JPEG 解码是确定的）：没有"解码 RGB 再重编码"的痕迹
    let again = image::load_from_memory(&back.map_img).expect("解记录里的底图").to_rgb8();
    assert_eq!(again.as_raw(), rgb.as_raw(), "记录里的底图与 8111 给的底图是同一张图");
    // 重编码成 JPEG 只会更大或更小但**不会等于**原字节（这里顺带证明没有二次压缩）
    let mut reencoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut reencoded, 90)
        .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .expect("重编码对照");
    assert!(
        bytes.ends_with(&jpeg),
        "底图段必须是原字节（重编码会变成 {} 字节，与原图 {} 字节不同）",
        reencoded.len(),
        jpeg.len()
    );
}
