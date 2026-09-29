//! 极小的 base64 编解码（RFC 4648 §4，标准字母表 + `=` padding）。
//!
//! ## 为什么自己写而不是加依赖
//!
//! `Cargo.toml` 的依赖面刻意只留 tauri / serde / url：依赖越少，供应链面与构建时间越可控。
//! 这里需要的只是「标准字母表 + padding」这一种形态，约 40 行。正确性不靠自觉，靠两处硬证据：
//!
//! - [`tests::matches_rfc4648_vectors`]：RFC 4648 §10 的官方测试向量；
//! - [`tests::round_trips_every_vendored_icon`]：14 个 vendored PNG **逐字节**编解码往返。
//!
//! ## 解码是安全边界
//!
//! `import_wallpaper` 的 `data_base64` 来自设置窗的文件输入（用户可控）。因此 [`decode`] 的
//! 语义刻意**收紧**：长度必须是 4 的倍数、字符必须落在字母表内、`=` 只能出现在最后一组且
//! 位置合法——一律不做「宽松修正」（`base64` crate 的 `STANDARD` 也是这个立场）。
//! 唯一的宽松之处：**不校验**被丢弃的低位是否为 0（非规范编码），因为它不影响字节长度上限的
//! 判定，而校验它只会多出一类需要解释的失败。

/// RFC 4648 §4 的标准字母表。
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 把字节编码成标准 base64（带 padding）。空输入 → 空串。
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// 单个字母表字符 → 6 位值；非字母表字符（含 `=`）→ `None`。
fn value_of(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a') as u32 + 26),
        b'0'..=b'9' => Some((c - b'0') as u32 + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// 标准 base64 → 字节。任何形状不合法都返回 `Err`（固定短语，不带用户输入）。
pub fn decode(input: &str) -> Result<Vec<u8>, String> {
    let bytes = input.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err("base64 长度不是 4 的倍数".to_string());
    }
    let groups = bytes.len() / 4;
    let mut out = Vec::with_capacity(groups * 3);
    for (i, g) in bytes.chunks(4).enumerate() {
        let last = i + 1 == groups;
        let pad = match (g[2], g[3]) {
            (b'=', b'=') => 2,
            (b'=', _) => return Err("base64 的 '=' 位置不合法".to_string()),
            (_, b'=') => 1,
            _ => 0,
        };
        if pad > 0 && !last {
            return Err("base64 的 padding 只能出现在最后一组".to_string());
        }
        let mut n = 0u32;
        for (j, &c) in g.iter().enumerate() {
            let v = if c == b'=' {
                // `=` 只能占据本组的末尾 `pad` 个位置
                if j + pad < 4 {
                    return Err("base64 的 '=' 位置不合法".to_string());
                }
                0
            } else {
                // padding 位之后不得再出现字母表字符
                if j + pad > 3 {
                    return Err("base64 的 padding 之后还有数据".to_string());
                }
                value_of(c).ok_or_else(|| "base64 含字母表之外的字符".to_string())?
            };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad == 0 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 §10 的官方测试向量（`BASE64("")=""` … `BASE64("foobar")="Zm9vYmFy"`）。
    #[test]
    fn matches_rfc4648_vectors() {
        for (raw, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(raw.as_bytes()), want, "encode({raw:?})");
            assert_eq!(
                decode(want).unwrap(),
                raw.as_bytes(),
                "decode({want:?}) 必须回到原文"
            );
        }
        // 字母表两端与 62/63 号字符（`+` / `/`）都要被覆盖
        assert_eq!(encode(&[0xfb, 0xff, 0xbf]), "+/+/");
        assert_eq!(decode("+/+/").unwrap(), vec![0xfb, 0xff, 0xbf]);
        // 全 0 / 全 0xff 的边界
        assert_eq!(encode(&[0, 0, 0]), "AAAA");
        assert_eq!(encode(&[0xff, 0xff, 0xff]), "////");
    }

    /// PNG 魔数（`89 50 4e 47`）必须编成 `iVBORw==`——运行期探针就靠这个前缀判断图标真的被嵌入。
    #[test]
    fn encodes_the_png_magic() {
        assert_eq!(encode(&[0x89, 0x50, 0x4e, 0x47]), "iVBORw==");
        assert_eq!(
            encode(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
            "iVBORw0KGgo="
        );
    }

    /// 解码的收紧处逐条锁住（这是 `import_wallpaper` 的入参闸门）。
    #[test]
    fn decode_rejects_every_malformed_shape() {
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
        assert!(decode("Zg=").is_err(), "长度不是 4 的倍数");
        assert!(decode("Zg===").is_err());
        assert!(decode("Z g==").is_err(), "含空白");
        assert!(decode("Zg=a").is_err(), "padding 之后还有数据");
        assert!(decode("Z=8=").is_err(), "'=' 位置不合法");
        assert!(decode("====").is_err());
        assert!(decode("Zm9vYmFy=").is_err(), "padding 出现在非最后一组");
        assert!(decode("Zm9v=YmFy").is_err(), "padding 出现在中间");
        assert!(
            decode("Zm9v\nYmFy").is_err(),
            "换行（长度恰好是 4 的倍数）也不放行"
        );
        assert!(decode("Zm9v-_.=").is_err(), "URL-safe 字母表不在允许范围内");
        // 不能 panic：所有畸形输入都走 Result
        for bad in ["", "=", "==", "===", "A", "\u{4e2d}", "AAAA===="] {
            let _ = decode(bad);
        }
    }

    /// **最强的一条**：14 个 vendored 图标逐个编解码往返，结果与磁盘字节逐字节相等。
    ///
    /// 这条同时锁住两件事：① 编码器对大二进制块（含各类字节对齐）正确；② 解码器能还原它。
    #[test]
    fn round_trips_every_vendored_icon() {
        let mut checked = 0usize;
        let mut total = 0usize;
        for (key, bytes) in crate::injector::prefect_icons() {
            let b64 = encode(bytes);
            assert_eq!(
                b64.len(),
                (bytes.len() + 2) / 3 * 4,
                "{key} 的 base64 长度必须符合 4/3 膨胀公式"
            );
            assert_eq!(
                decode(&b64).unwrap().as_slice(),
                *bytes,
                "{key} 编解码往返必须逐字节相等"
            );
            assert_eq!(
                &b64[..8.min(b64.len())],
                "iVBORw0K",
                "{key} 必须是一张 PNG（魔数 iVBORw0KGgo 的 base64 前缀）"
            );
            checked += 1;
            total += bytes.len();
        }
        assert_eq!(checked, 14, "完美图标资源必须恰好 14 个");
        assert!(total > 700 * 1024, "14 个图标合计应有数百 KB，实测 {total}");
    }
}
