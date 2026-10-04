//! Conservative FeiQ text compatibility; this module does not interpret rich text.

const FONT_MARKER: &str = "{/font;";
const MAX_FONT_SUFFIX_BYTES: usize = 512;

fn trim_spaces(text: &str) -> &str {
    text.trim_matches(|c| c == ' ' || c == '\t')
}

fn signed_integer(text: &str) -> Option<i32> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn valid_font_fields(mut fields: &str) -> bool {
    if fields
        .chars()
        .any(|c| matches!(c, '{' | '}' | ';' | '\r' | '\n' | '\0'))
    {
        return false;
    }
    // LOGFONT: five LONG values, then eight BYTE values, then the face name.
    for index in 0..13 {
        fields = trim_spaces(fields);
        let Some(end) = fields.find(|c| c == ' ' || c == '\t') else {
            return false;
        };
        let Some(value) = signed_integer(&fields[..end]) else {
            return false;
        };
        if index >= 5 && !(0..=255).contains(&value) {
            return false;
        }
        fields = &fields[end + 1..];
    }
    fields = trim_spaces(fields);
    // Only the last token is the color. A font face may itself contain spaces.
    let Some(color_start) = fields.rfind(|c| c == ' ' || c == '\t') else {
        return false;
    };
    if trim_spaces(&fields[..color_start]).is_empty() {
        return false;
    }
    let color = &fields[color_start + 1..];
    !color.is_empty()
        && color.bytes().all(|byte| byte.is_ascii_digit())
        && color.parse::<u32>().is_ok()
}

/// Remove one complete, valid trailing `{/font;...;}` block, retaining the exact
/// preceding text. Invalid, partial or non-trailing markup is ordinary text.
pub fn strip_feiq_font_suffix(body: &str) -> String {
    let Some(start) = body.rfind(FONT_MARKER) else {
        return body.to_owned();
    };
    let suffix = &body[start..];
    if suffix.len() > MAX_FONT_SUFFIX_BYTES || suffix.len() < FONT_MARKER.len() + 2 {
        return body.to_owned();
    }
    let Some(fields) = suffix[FONT_MARKER.len()..].strip_suffix(";}") else {
        return body.to_owned();
    };
    if valid_font_fields(fields) {
        body[..start].to_owned()
    } else {
        body.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIELDS: &str = "-8 0 0 0 400 0 0 0 1 0 0 2 32 微软雅黑 8404992";

    fn suffix(fields: &str) -> String {
        format!("{{/font;{fields};}}")
    }

    #[test]
    fn removes_the_verified_feiq_font_suffix() {
        let body = format!("1122{}", suffix(FIELDS));
        assert_eq!(strip_feiq_font_suffix(&body), "1122");
    }

    #[test]
    fn preserves_all_text_whitespace_and_emoji_tokens() {
        let text = "  你好\r\n第二行 <msg><emoji type=\"1\" id=\"e2_02\" /></msg> \n";
        assert_eq!(
            strip_feiq_font_suffix(&format!("{text}{}", suffix(FIELDS))),
            text
        );
    }

    #[test]
    fn handles_negative_long_fields_and_space_separated_font_faces() {
        let fields = "-12 -1 -2 -3 700 1 1 0 134 0 0 2 32 Microsoft YaHei UI 4294967295";
        assert_eq!(
            strip_feiq_font_suffix(&format!("字{}", suffix(fields))),
            "字"
        );
    }

    #[test]
    fn permits_spaces_and_tabs_between_fields() {
        let fields = "\t-8\t0  0 0 400 0 0 0 1 0 0 2 32\t 微软雅黑\t8404992\t";
        assert_eq!(
            strip_feiq_font_suffix(&format!("ok{}", suffix(fields))),
            "ok"
        );
    }

    #[test]
    fn removes_only_one_trailing_block_and_never_middle_markup() {
        let tag = suffix(FIELDS);
        assert_eq!(
            strip_feiq_font_suffix(&format!("before{tag}after")),
            format!("before{tag}after")
        );
        assert_eq!(
            strip_feiq_font_suffix(&format!("before{tag} ")),
            format!("before{tag} ")
        );
        assert_eq!(strip_feiq_font_suffix(&format!("{tag}{tag}")), tag);
    }

    #[test]
    fn leaves_incomplete_or_empty_markup_untouched() {
        for body in [
            "",
            "plain {braces}",
            "text{/font;}",
            "text{/font;;}",
            "text{/font;1",
            "{/font;",
        ] {
            assert_eq!(strip_feiq_font_suffix(body), body);
        }
        let mut body = format!("text{}", suffix(FIELDS));
        body.pop();
        assert_eq!(strip_feiq_font_suffix(&body), body);
    }

    #[test]
    fn malformed_numeric_fields_and_missing_face_are_not_removed() {
        for fields in [
            "-8 0 0 0 400 0 0 0 1 0 0 2 微软雅黑 8404992", // twelve numeric fields
            "bad 0 0 0 400 0 0 0 1 0 0 2 32 微软雅黑 8404992",
            "+8 0 0 0 400 0 0 0 1 0 0 2 32 微软雅黑 8404992",
            "2147483648 0 0 0 400 0 0 0 1 0 0 2 32 微软雅黑 8404992",
            "-2147483649 0 0 0 400 0 0 0 1 0 0 2 32 微软雅黑 8404992",
            "-8 0 0 0 400 256 0 0 1 0 0 2 32 微软雅黑 8404992",
            "-8 0 0 0 400 -1 0 0 1 0 0 2 32 微软雅黑 8404992",
            "-8 0 0 0 400 0 0 0 1 0 0 2 32 8404992", // no font face
            "-8 0 0 0 400 0 0 0 1 0 0 2 32 微软雅黑 -1",
            "-8 0 0 0 400 0 0 0 1 0 0 2 32 微软雅黑 +1",
            "-8 0 0 0 400 0 0 0 1 0 0 2 32 微软雅黑 4294967296",
            "-8 0 0 0 400 0 0 0 1 0 0 2 32 微软雅黑 blue",
            "-8 0 0 0 400 0 0 0 1 0 0 2 32 字体;名 8404992",
            "-8 0 0 0 400 0 0 0 1 0 0 2 32 字体\n名 8404992",
            "-8 0 0 0 400 0 0 0 1 0 0 2 32 字体\0名 8404992",
        ] {
            let body = format!("keep{}", suffix(fields));
            assert_eq!(strip_feiq_font_suffix(&body), body, "stripped {fields:?}");
        }
    }

    #[test]
    fn valid_signed_boundaries_are_supported() {
        let fields = "-2147483648 2147483647 0 0 400 0 0 0 255 0 0 2 32 Font 0";
        assert_eq!(strip_feiq_font_suffix(&suffix(fields)), "");
    }

    #[test]
    fn overlong_suffix_and_multibyte_prefix_do_not_panic() {
        let fields = FIELDS.replace("微软雅黑", &"字".repeat(200));
        let body = format!("🦀 中文{}", suffix(&fields));
        assert_eq!(strip_feiq_font_suffix(&body), body);
        let body = format!("🦀 中文{}", suffix(FIELDS));
        assert_eq!(strip_feiq_font_suffix(&body), "🦀 中文");
    }
}
