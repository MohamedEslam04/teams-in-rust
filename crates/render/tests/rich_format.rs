//! Composer formatting data layer: decode real-shaped Teams HTML, encode the
//! HTML Teams web sends, Markdown-style input, sanitization. All text is
//! synthetic; tags and attributes follow what Teams really sends.

use render::{
    FontSize, ListItem, RichBlock, RichDoc, RichList, RichTable, Span, Style, TableCell,
    markdown_to_composer_html, markdown_to_rich, parse_rich_html,
};

// ---- helpers ----

fn sp(text: &str) -> Span {
    Span {
        text: text.into(),
        style: Style::default(),
        link: None,
        mention: None,
    }
}

fn st(text: &str, f: impl Fn(&mut Style)) -> Span {
    let mut s = sp(text);
    f(&mut s.style);
    s
}

fn link(text: &str, url: &str) -> Span {
    Span {
        link: Some(url.into()),
        ..sp(text)
    }
}

fn para(spans: Vec<Span>) -> RichBlock {
    RichBlock::Paragraph(spans)
}

fn p(text: &str) -> RichBlock {
    para(vec![sp(text)])
}

fn item(text: &str) -> ListItem {
    ListItem {
        spans: vec![sp(text)],
        children: vec![],
    }
}

fn list(ordered: bool, items: Vec<ListItem>) -> RichList {
    RichList {
        ordered,
        start: None,
        items,
    }
}

fn code(language: Option<&str>, text: &str) -> RichBlock {
    RichBlock::Code {
        language: language.map(Into::into),
        text: text.into(),
    }
}

fn doc(blocks: Vec<RichBlock>) -> RichDoc {
    RichDoc { blocks }
}

fn dec(html: &str) -> Vec<RichBlock> {
    parse_rich_html(html).blocks
}

fn round_trip(d: RichDoc) {
    let html = d.to_html();
    assert_eq!(parse_rich_html(&html), d, "via {html}");
}

// ---- decode ----

#[test]
fn decode_headings_h1_to_h3_and_deeper_levels_map_to_3() {
    assert_eq!(
        dec("<h1>One</h1><h2>Two</h2><h3>Three</h3><h5>Five</h5>"),
        vec![
            RichBlock::Heading {
                level: 1,
                spans: vec![sp("One")]
            },
            RichBlock::Heading {
                level: 2,
                spans: vec![sp("Two")]
            },
            RichBlock::Heading {
                level: 3,
                spans: vec![sp("Three")]
            },
            RichBlock::Heading {
                level: 3,
                spans: vec![sp("Five")]
            },
        ]
    );
}

#[test]
fn decode_heading_keeps_inline_formatting() {
    assert_eq!(
        dec("<h2>Plan <i>b</i></h2>"),
        vec![RichBlock::Heading {
            level: 2,
            spans: vec![sp("Plan "), st("b", |s| s.italic = true)]
        }]
    );
}

#[test]
fn decode_basic_inline_styles() {
    assert_eq!(
        dec("<p>a <strong>b</strong> <b>c</b> <i>d</i> <em>e</em> <u>f</u> <code>g</code></p>"),
        vec![para(vec![
            sp("a "),
            st("b", |s| s.bold = true),
            sp(" "),
            st("c", |s| s.bold = true),
            sp(" "),
            st("d", |s| s.italic = true),
            sp(" "),
            st("e", |s| s.italic = true),
            sp(" "),
            st("f", |s| s.underline = true),
            sp(" "),
            st("g", |s| s.code = true),
        ])]
    );
}

#[test]
fn decode_strikethrough_in_all_shapes() {
    for html in [
        "<p><s>x</s></p>",
        "<p><strike>x</strike></p>",
        "<p><del>x</del></p>",
        r#"<p><span style="text-decoration:line-through;">x</span></p>"#,
        r#"<p><span style="text-decoration: underline line-through">x</span></p>"#,
    ] {
        let spans = match &dec(html)[0] {
            RichBlock::Paragraph(s) => s.clone(),
            other => panic!("{other:?}"),
        };
        assert!(spans[0].style.strike, "{html}");
    }
}

#[test]
fn decode_underline_via_text_decoration() {
    assert_eq!(
        dec(r#"<p><span style="text-decoration:underline">x</span></p>"#),
        vec![para(vec![st("x", |s| s.underline = true)])]
    );
}

#[test]
fn decode_text_color_in_hex_and_rgb() {
    let red = |t: &str| st(t, |s| s.color = Some("#ff0000".into()));
    assert_eq!(
        dec(
            r#"<p><span style="color:#FF0000;">a</span><span style="color: rgb(255, 0, 0)">b</span><span style="color:#f00">c</span></p>"#
        ),
        // adjacent equal styles merge
        vec![para(vec![red("abc")])]
    );
}

#[test]
fn decode_highlight_background_color_and_palette_span() {
    assert_eq!(
        dec(r#"<p><span style="background-color:rgb(1, 2, 3)">a</span></p>"#),
        vec![para(
            vec![st("a", |s| s.highlight = Some("#010203".into()))]
        )]
    );
    // Teams palette highlight: itemtype + slug + fallback style.
    assert_eq!(
        dec(
            r##"<p><span itemtype="http://schema.skype.com/HighlightColor" itemid="marigold" style="background-color:#FDD472">a</span></p>"##
        ),
        vec![para(
            vec![st("a", |s| s.highlight = Some("#fdd472".into()))]
        )]
    );
    // Slug without a usable style falls back to the palette color.
    assert_eq!(
        dec(
            r#"<p><span itemtype="http://schema.skype.com/HighlightColor" itemid="seafoam">a</span></p>"#
        ),
        vec![para(
            vec![st("a", |s| s.highlight = Some("#82cda8".into()))]
        )]
    );
}

#[test]
fn decode_font_size() {
    assert_eq!(
        dec(
            r#"<p><span style="font-size:x-large;">a</span> <span style="font-size:xx-small">b</span> <span style="font-size:inherit">c</span></p>"#
        ),
        vec![para(vec![
            st("a", |s| s.size = Some(FontSize::Large)),
            sp(" "),
            st("b", |s| s.size = Some(FontSize::Small)),
            sp(" c"),
        ])]
    );
}

#[test]
fn decode_composed_styles_nest() {
    assert_eq!(
        dec(
            r#"<p><strong><i><span style="color:#00ff00;"><span style="background-color:#000000;">x</span></span></i></strong></p>"#
        ),
        vec![para(vec![st("x", |s| {
            s.bold = true;
            s.italic = true;
            s.color = Some("#00ff00".into());
            s.highlight = Some("#000000".into());
        })])]
    );
}

#[test]
fn decode_code_block_teams_composer_shape() {
    assert_eq!(
        dec(
            r#"<pre class="language-python skipProofing" spellcheck="false"><code>def f():<br>    return 1</code></pre>"#
        ),
        vec![code(Some("python"), "def f():\n    return 1")]
    );
}

#[test]
fn decode_code_block_server_echo_shape() {
    // What Teams stores for a sent code block: no skipProofing, an itemid.
    assert_eq!(
        dec(
            r#"<p></p><pre class="language-plaintext" itemid="abc123"><code>one<br>two</code></pre>"#
        ),
        vec![code(None, "one\ntwo")]
    );
}

#[test]
fn decode_code_block_without_language_and_with_literal_newlines() {
    assert_eq!(
        dec("<pre><code>a &lt; b\n  c</code></pre>"),
        vec![code(None, "a < b\n  c")]
    );
    assert_eq!(
        dec(r#"<pre><code class="language-Rust">x</code></pre>"#),
        vec![code(Some("rust"), "x")]
    );
}

#[test]
fn decode_blockquote_with_br_and_with_paragraphs() {
    assert_eq!(
        dec("<blockquote>q1<br>q2</blockquote>"),
        vec![RichBlock::Quote(vec![sp("q1\nq2")])]
    );
    assert_eq!(
        dec("<blockquote><p>q1</p><p>q2 <b>b</b></p></blockquote>"),
        vec![RichBlock::Quote(vec![
            sp("q1\nq2 "),
            st("b", |s| s.bold = true)
        ])]
    );
}

#[test]
fn decode_reply_blockquote_becomes_quote_of_the_preview() {
    let html = concat!(
        r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="1731234567890">"#,
        r#"<strong itemprop="mri" itemid="8:orgid:abc">Sara</strong>"#,
        r#"<span itemprop="time" itemid="1731234999000"></span>"#,
        r#"<p itemprop="preview">orig text</p></blockquote><p>reply</p>"#
    );
    assert_eq!(
        dec(html),
        vec![RichBlock::Quote(vec![sp("orig text")]), p("reply")]
    );
}

#[test]
fn decode_nested_bullet_and_numbered_lists() {
    let expected = RichBlock::List(list(
        false,
        vec![
            ListItem {
                spans: vec![sp("a")],
                children: vec![list(true, vec![item("a1"), item("a2")])],
            },
            item("b"),
        ],
    ));
    assert_eq!(
        dec("<ul><li>a<ol><li>a1</li><li>a2</li></ol></li><li>b</li></ul>"),
        vec![expected.clone()]
    );
    // Whitespace and <p> inside <li> (as other clients send it).
    assert_eq!(
        dec("<ul>\n <li><p>a</p>\n <ol><li><p>a1</p></li><li>a2</li></ol></li>\n <li>b</li></ul>"),
        vec![expected]
    );
}

#[test]
fn decode_ordered_list_start() {
    assert_eq!(
        dec(r#"<ol start="3"><li>c</li><li>d</li></ol><ol start="1"><li>x</li></ol>"#),
        vec![
            RichBlock::List(RichList {
                ordered: true,
                start: Some(3),
                items: vec![item("c"), item("d")]
            }),
            RichBlock::List(list(true, vec![item("x")])),
        ]
    );
}

#[test]
fn decode_list_item_formatting_and_links() {
    assert_eq!(
        dec(r#"<ul><li><b>x</b> <a href="https://e.test/p">y</a></li></ul>"#),
        vec![RichBlock::List(list(
            false,
            vec![ListItem {
                spans: vec![
                    st("x", |s| s.bold = true),
                    sp(" "),
                    link("y", "https://e.test/p")
                ],
                children: vec![]
            }]
        ))]
    );
}

#[test]
fn decode_table_composer_shape() {
    let html = concat!(
        r#"<figure class="table"><table class="copy-paste-table"><tbody>"#,
        "<tr><td>a</td><td>&nbsp;</td></tr>",
        "<tr><td><p>x</p><p>y</p></td><td><b>z</b></td></tr>",
        "</tbody></table></figure><p>after</p>"
    );
    assert_eq!(
        dec(html),
        vec![
            RichBlock::Table(RichTable {
                rows: vec![
                    vec![TableCell::new(vec![sp("a")]), TableCell::new(vec![])],
                    vec![
                        TableCell::new(vec![sp("x\ny")]),
                        TableCell::new(vec![st("z", |s| s.bold = true)])
                    ],
                ]
            }),
            p("after")
        ]
    );
}

#[test]
fn decode_table_header_row_and_spans() {
    let html = concat!(
        "<table><thead><tr><th>H1</th><th colspan=\"2\">H2</th></tr></thead>",
        "<tbody><tr><td rowspan=\"2\">r</td><td>c</td><td>d</td></tr>",
        "<tr><td colspan=\"0\">e</td><td colspan=\"x\">f</td></tr></tbody></table>"
    );
    let header = |t: &str, colspan| TableCell {
        header: true,
        colspan,
        rowspan: 1,
        spans: vec![sp(t)],
    };
    assert_eq!(
        dec(html),
        vec![RichBlock::Table(RichTable {
            rows: vec![
                vec![header("H1", 1), header("H2", 2)],
                vec![
                    TableCell {
                        rowspan: 2,
                        ..TableCell::new(vec![sp("r")])
                    },
                    TableCell::new(vec![sp("c")]),
                    TableCell::new(vec![sp("d")]),
                ],
                // invalid spans fall back to 1
                vec![TableCell::new(vec![sp("e")]), TableCell::new(vec![sp("f")])],
            ]
        })]
    );
}

#[test]
fn decode_links_in_teams_shapes_and_bad_schemes() {
    assert_eq!(
        dec(
            r#"<p><a href="https://e.test/a?x=1&amp;y=2" target="_blank" rel="noreferrer noopener" title="t" itemtype="http://schema.skype.com/HyperLink" itemid="9">go</a></p>"#
        ),
        vec![para(vec![link("go", "https://e.test/a?x=1&y=2")])]
    );
    assert_eq!(
        dec(
            r#"<p><a href="javascript:alert(1)">x</a> <a href=" JaVaScRiPt:y">z</a> <a href="mailto:a@b.test">m</a></p>"#
        ),
        vec![para(vec![sp("x z "), link("m", "mailto:a@b.test")])]
    );
}

#[test]
fn decode_teams_emoticon_ids_through_the_catalog() {
    let blocks = dec(concat!(
        r#"<p>a<img itemscope itemtype="http://schema.skype.com/Emoji" itemid="muscle-tone1" src="https://e.test/m.png">"#,
        r#"b<emoji id="cwl" title="Crying with laughter"></emoji>c</p>"#,
    ));
    assert_eq!(blocks, vec![para(vec![sp("a💪🏻b😂c")])]);
}

#[test]
fn decode_rule_mention_emoji_image_and_line_breaks() {
    let blocks = dec(concat!(
        r#"<p>hi <span itemscope itemtype="http://schema.skype.com/Mention" itemid="0">Sam Doe</span>"#,
        r#" <span class="animated-emoticon-50-cool"><img itemscope itemtype="http://schema.skype.com/Emoji" itemid="cool" alt="😎" src="https://e.test/cool.png"></span>"#,
        "<br>next</p><hr>",
        r#"<p><img src="https://e.test/i.png" width="10" height="20"></p>"#,
    ));
    assert_eq!(
        blocks,
        vec![
            para(vec![
                sp("hi "),
                Span {
                    mention: Some("0".into()),
                    ..sp("Sam Doe")
                },
                sp(" 😎\nnext"),
            ]),
            RichBlock::Rule,
            RichBlock::Image {
                url: "https://e.test/i.png".into(),
                width: Some(10),
                height: Some(20)
            },
        ]
    );
}

#[test]
fn decode_plain_text_and_blank_lines() {
    assert_eq!(dec("a &amp; b\n\nc"), vec![p("a & b"), p("c")]);
    // `<p>&nbsp;</p>` is the composer's blank line; `<p></p>` is nothing.
    assert_eq!(
        dec("<p>a</p><p>&nbsp;</p><p></p><p>b</p>"),
        vec![p("a"), para(vec![]), p("b")]
    );
}

// ---- encode (exact Teams composer HTML) ----

#[test]
fn encode_inline_styles() {
    let d = doc(vec![para(vec![
        sp("a "),
        st("b", |s| s.bold = true),
        sp(" "),
        st("i", |s| s.italic = true),
        sp(" "),
        st("u", |s| s.underline = true),
        sp(" "),
        st("s", |s| s.strike = true),
        sp(" "),
        st("c", |s| s.code = true),
    ])]);
    assert_eq!(
        d.to_html(),
        "<p>a <strong>b</strong> <i>i</i> <u>u</u> <s>s</s> <code>c</code></p>"
    );
}

#[test]
fn encode_combined_styles_color_highlight_size_link() {
    let s = st("x", |s| {
        s.bold = true;
        s.italic = true;
        s.color = Some("#FF0000".into());
        s.highlight = Some("#123456".into());
        s.size = Some(FontSize::Large);
    });
    let s = Span {
        link: Some("https://e.test/".into()),
        ..s
    };
    assert_eq!(
        doc(vec![para(vec![s])]).to_html(),
        concat!(
            r#"<p><a href="https://e.test/"><strong><i>"#,
            r#"<span style="color:#ff0000;"><span style="background-color:#123456;">"#,
            r#"<span style="font-size:x-large;">x</span></span></span></i></strong></a></p>"#
        )
    );
}

#[test]
fn encode_palette_highlight_uses_teams_highlight_span() {
    let s = st("x", |s| s.highlight = Some("#fdd472".into()));
    assert_eq!(
        doc(vec![para(vec![s])]).to_html(),
        r##"<p><span itemtype="http://schema.skype.com/HighlightColor" itemid="marigold" style="background-color:#FDD472">x</span></p>"##
    );
}

#[test]
fn encode_small_size_and_mention_and_line_break() {
    let d = doc(vec![para(vec![
        st("s", |s| s.size = Some(FontSize::Small)),
        sp(" "),
        Span {
            mention: Some("0".into()),
            ..sp("Sam")
        },
        sp("\nnext"),
    ])]);
    assert_eq!(
        d.to_html(),
        concat!(
            r#"<p><span style="font-size:xx-small;">s</span> "#,
            r#"<span itemscope itemtype="http://schema.skype.com/Mention" itemid="0">Sam</span>"#,
            "<br>next</p>"
        )
    );
}

#[test]
fn encode_blocks() {
    let d = doc(vec![
        RichBlock::Heading {
            level: 2,
            spans: vec![sp("T")],
        },
        RichBlock::Quote(vec![sp("q1\nq2")]),
        RichBlock::Rule,
        para(vec![]),
        code(Some("py"), "a\n  b"),
        code(None, "x"),
    ]);
    assert_eq!(
        d.to_html(),
        concat!(
            "<h2>T</h2>",
            "<blockquote>q1<br>q2</blockquote>",
            "<hr>",
            "<p>&nbsp;</p>",
            r#"<pre class="language-py skipProofing" spellcheck="false"><code>a<br>  b</code></pre>"#,
            r#"<pre class="language-plaintext skipProofing" spellcheck="false"><code>x</code></pre>"#,
        )
    );
}

#[test]
fn encode_lists_nested_and_numbered() {
    let d = doc(vec![
        RichBlock::List(list(
            false,
            vec![
                ListItem {
                    spans: vec![sp("a")],
                    children: vec![list(true, vec![item("a1")])],
                },
                item("b"),
            ],
        )),
        RichBlock::List(RichList {
            ordered: true,
            start: Some(3),
            items: vec![item("c")],
        }),
        RichBlock::List(RichList {
            ordered: true,
            start: Some(1),
            items: vec![item("d")],
        }),
    ]);
    assert_eq!(
        d.to_html(),
        concat!(
            "<ul><li>a<ol><li>a1</li></ol></li><li>b</li></ul>",
            r#"<ol start="3"><li>c</li></ol>"#,
            "<ol><li>d</li></ol>",
        )
    );
}

#[test]
fn encode_table_like_teams_composer() {
    let d = doc(vec![RichBlock::Table(RichTable {
        rows: vec![
            vec![
                TableCell {
                    header: true,
                    ..TableCell::new(vec![sp("H")])
                },
                TableCell {
                    header: true,
                    colspan: 2,
                    ..TableCell::new(vec![sp("I")])
                },
            ],
            vec![
                TableCell::new(vec![sp("x\ny")]),
                TableCell {
                    rowspan: 2,
                    ..TableCell::new(vec![])
                },
                TableCell::new(vec![sp("a<b")]),
            ],
        ],
    })]);
    assert_eq!(
        d.to_html(),
        concat!(
            r#"<figure class="table"><table class="copy-paste-table">"#,
            r#"<thead><tr><th>H</th><th colspan="2">I</th></tr></thead>"#,
            r#"<tbody><tr><td><p>x</p><p>y</p></td><td rowspan="2">&nbsp;</td><td>a&lt;b</td></tr></tbody>"#,
            "</table></figure>"
        )
    );
}

#[test]
fn encode_text_with_spaces_and_special_chars() {
    let d = doc(vec![p("a  b & <c> \"d\"")]);
    assert_eq!(
        d.to_html(),
        "<p>a &nbsp;b &amp; &lt;c&gt; &quot;d&quot;</p>"
    );
}

// ---- round trips ----

#[test]
fn round_trip_each_inline_format() {
    for f in [
        (|s: &mut Style| s.bold = true) as fn(&mut Style),
        |s| s.italic = true,
        |s| s.underline = true,
        |s| s.strike = true,
        |s| s.code = true,
        |s| s.color = Some("#aa0011".into()),
        |s| s.highlight = Some("#123456".into()),
        |s| s.highlight = Some("#df9299".into()),
        |s| s.size = Some(FontSize::Large),
        |s| s.size = Some(FontSize::Small),
    ] {
        round_trip(doc(vec![para(vec![sp("a "), st("x", f), sp(" b")])]));
    }
}

#[test]
fn round_trip_all_styles_together_links_and_mentions() {
    round_trip(doc(vec![para(vec![
        st("all", |s| {
            s.bold = true;
            s.italic = true;
            s.underline = true;
            s.strike = true;
            s.color = Some("#010203".into());
            s.highlight = Some("#fdd472".into());
            s.size = Some(FontSize::Small);
        }),
        sp(" "),
        Span {
            link: Some("https://e.test/?a=1&b=2".into()),
            ..st("l", |s| s.bold = true)
        },
        sp(" "),
        Span {
            mention: Some("1".into()),
            ..sp("Sam Doe")
        },
        sp(" tail\nline2"),
    ])]));
}

#[test]
fn round_trip_headings_quote_rule_code_blank_lines() {
    round_trip(doc(vec![
        RichBlock::Heading {
            level: 1,
            spans: vec![sp("One")],
        },
        RichBlock::Heading {
            level: 3,
            spans: vec![st("Three", |s| s.italic = true)],
        },
        p("para"),
        para(vec![]),
        RichBlock::Quote(vec![sp("q1\nq2 "), st("b", |s| s.bold = true)]),
        RichBlock::Rule,
        code(Some("rust"), "fn a() {\n    <b>&\n}"),
        code(None, "plain  text\n\n  indented"),
    ]));
}

#[test]
fn round_trip_lists() {
    round_trip(doc(vec![
        RichBlock::List(list(
            false,
            vec![
                ListItem {
                    spans: vec![st("a", |s| s.bold = true), sp(" x")],
                    children: vec![
                        list(true, vec![item("a1"), item("a2")]),
                        list(false, vec![item("a3")]),
                    ],
                },
                item("b"),
            ],
        )),
        RichBlock::List(RichList {
            ordered: true,
            start: Some(5),
            items: vec![item("five"), item("six")],
        }),
    ]));
}

#[test]
fn round_trip_table_with_header_spans_and_multiline_cells() {
    round_trip(doc(vec![RichBlock::Table(RichTable {
        rows: vec![
            vec![
                TableCell {
                    header: true,
                    ..TableCell::new(vec![sp("A")])
                },
                TableCell {
                    header: true,
                    colspan: 2,
                    ..TableCell::new(vec![sp("B")])
                },
            ],
            vec![
                TableCell::new(vec![sp("one\ntwo")]),
                TableCell::new(vec![]),
                TableCell {
                    rowspan: 2,
                    ..TableCell::new(vec![st("z", |s| s.code = true)])
                },
            ],
            vec![
                TableCell::new(vec![sp("<&>")]),
                TableCell::new(vec![sp("q")]),
            ],
        ],
    })]));
}

#[test]
fn round_trip_image() {
    round_trip(doc(vec![RichBlock::Image {
        url: "https://e.test/i.png".into(),
        width: Some(30),
        height: None,
    }]));
}

#[test]
fn encode_decode_encode_is_stable_for_real_shaped_input() {
    let html = concat!(
        r#"<h2>Title</h2><p>a <strong>b</strong> <span style="color:rgb(255, 0, 0)">c</span></p>"#,
        r#"<ul><li>x<ol><li>y</li></ol></li></ul>"#,
        r#"<pre class="language-sql skipProofing" spellcheck="false"><code>select 1;<br>select 2;</code></pre>"#,
    );
    let once = parse_rich_html(html).to_html();
    assert_eq!(parse_rich_html(&once).to_html(), once);
}

// ---- markdown-ish input ----

#[test]
fn markdown_inline_to_rich() {
    assert_eq!(
        markdown_to_rich("**b** _i_ ~~s~~ `c` [l](https://e.test/) ++u++").blocks,
        vec![para(vec![
            st("b", |s| s.bold = true),
            sp(" "),
            st("i", |s| s.italic = true),
            sp(" "),
            st("s", |s| s.strike = true),
            sp(" "),
            st("c", |s| s.code = true),
            sp(" "),
            link("l", "https://e.test/"),
            sp(" "),
            st("u", |s| s.underline = true),
        ])]
    );
}

#[test]
fn markdown_color_highlight_size_markers() {
    assert_eq!(
        markdown_to_rich("{color:#ff0000}r{/} {bg:#fdd472}h{/} {size:large}L{/}").blocks,
        vec![para(vec![
            st("r", |s| s.color = Some("#ff0000".into())),
            sp(" "),
            st("h", |s| s.highlight = Some("#fdd472".into())),
            sp(" "),
            st("L", |s| s.size = Some(FontSize::Large)),
        ])]
    );
}

#[test]
fn markdown_blocks_to_rich() {
    let md = "# H1\n### H3\ntext\n\n> q1\n> q2\n\n- a\n  1. a1\n- b\n\n3. c\n\n```rust\nfn x() {}\n```\n---";
    assert_eq!(
        markdown_to_rich(md).blocks,
        vec![
            RichBlock::Heading {
                level: 1,
                spans: vec![sp("H1")]
            },
            RichBlock::Heading {
                level: 3,
                spans: vec![sp("H3")]
            },
            p("text"),
            para(vec![]),
            RichBlock::Quote(vec![sp("q1\nq2")]),
            RichBlock::List(list(
                false,
                vec![
                    ListItem {
                        spans: vec![sp("a")],
                        children: vec![list(true, vec![item("a1")])]
                    },
                    item("b")
                ]
            )),
            RichBlock::List(list(true, vec![item("c")])),
            code(Some("rust"), "fn x() {}"),
            RichBlock::Rule,
        ]
    );
}

#[test]
fn markdown_table_has_a_header_row() {
    let blocks = markdown_to_rich("| A | B |\n|---|---|\n| 1 | **2** |").blocks;
    assert_eq!(
        blocks,
        vec![RichBlock::Table(RichTable {
            rows: vec![
                vec![
                    TableCell {
                        header: true,
                        ..TableCell::new(vec![sp("A")])
                    },
                    TableCell {
                        header: true,
                        ..TableCell::new(vec![sp("B")])
                    },
                ],
                vec![
                    TableCell::new(vec![sp("1")]),
                    TableCell::new(vec![st("2", |s| s.bold = true)])
                ],
            ]
        })]
    );
}

#[test]
fn markdown_to_composer_html_uses_teams_tags() {
    assert_eq!(
        markdown_to_composer_html("**b** _i_\n```py\nprint(1)\n```"),
        concat!(
            "<p><strong>b</strong> <i>i</i></p>",
            r#"<pre class="language-py skipProofing" spellcheck="false"><code>print(1)</code></pre>"#
        )
    );
}

#[test]
fn right_to_left_blocks_carry_their_direction() {
    assert_eq!(
        markdown_to_composer_html("Hi team\nحاضر\nok تمام"),
        r#"<p dir="ltr">Hi team</p><p dir="rtl">حاضر</p><p dir="ltr">ok تمام</p>"#
    );
    // Formatting does not hide the direction, and blocks without text stay plain.
    assert_eq!(
        markdown_to_composer_html("**حاضر**\n\n- one\n- تمام"),
        concat!(
            r#"<p dir="rtl"><strong>حاضر</strong></p><p>&nbsp;</p>"#,
            r#"<ul><li dir="ltr">one</li><li dir="rtl">تمام</li></ul>"#
        )
    );
    assert_eq!(
        doc(vec![
            RichBlock::Heading {
                level: 2,
                spans: vec![sp("عنوان")]
            },
            RichBlock::Quote(vec![sp("quote")]),
        ])
        .to_html(),
        r#"<h2 dir="rtl">عنوان</h2><blockquote dir="ltr">quote</blockquote>"#
    );
}

#[test]
fn left_to_right_documents_have_no_direction() {
    assert_eq!(
        markdown_to_composer_html("one\ntwo\n- x"),
        "<p>one</p><p>two</p><ul><li>x</li></ul>"
    );
}

#[test]
fn markdown_round_trips_through_html() {
    let d = markdown_to_rich("# T\n**b** and `c`\n- x\n- y\n```sh\nls\n```");
    assert_eq!(parse_rich_html(&d.to_html()), d);
}

#[test]
fn markdown_to_teams_html_keeps_legacy_output_for_language_fences() {
    // The calendar path keeps its plain `<pre><code>`.
    assert_eq!(
        render::markdown_to_teams_html("```rust\nx\n```"),
        "<pre><code>x</code></pre>"
    );
}

// ---- sanitization ----

#[test]
fn hostile_html_does_not_survive_a_round_trip() {
    let hostile = concat!(
        r#"<p onclick="evil()" style="color:red;background:url(javascript:x)">a<script>alert(1)</script>"#,
        r#"<img src="x" onerror="alert(2)"><style>p{x:y}</style>"#,
        r#"<a href="javascript:alert(3)" onmouseover="z()">link</a>"#,
        r#"<span style="color:expression(alert(4))">e</span>"#,
        r#"<span style="color:red;font-family:x;position:fixed">f</span>"#,
        r#"<iframe src="https://evil.test"></iframe><object data="x"></object></p>"#,
    );
    let html = parse_rich_html(hostile).to_html();
    for bad in [
        "script",
        "onclick",
        "onerror",
        "onmouseover",
        "javascript",
        "expression",
        "<style",
        "iframe",
        "object",
        "position",
        "font-family",
        "url(",
        "<img",
    ] {
        assert!(!html.contains(bad), "{bad} survived in {html}");
    }
    assert!(html.contains("link"));
}

#[test]
fn color_values_are_validated_on_decode() {
    for bad in [
        "red;background:url(x)",
        "expression(alert(1))",
        "var(--x)",
        "#12345",
        "#gggggg",
        "rgb(300, 0, 0)",
        "rgb(1,2)",
        "url(x)",
        "transparent",
        "rgba(1, 2, 3, 0)",
    ] {
        let html = format!(r#"<p><span style="color:{bad}">x</span></p>"#);
        let spans = match &dec(&html)[0] {
            RichBlock::Paragraph(s) => s.clone(),
            o => panic!("{o:?}"),
        };
        assert_eq!(spans, vec![sp("x")], "{bad}");
    }
}

#[test]
fn encoder_revalidates_model_values() {
    let evil = Span {
        link: Some("javascript:alert(1)".into()),
        ..st("<script>x</script>", |s| {
            s.color = Some("red;background:url(javascript:1)".into());
            s.highlight = Some("\"><script>".into());
        })
    };
    let d = doc(vec![
        para(vec![evil]),
        code(
            Some("x\"><script>"),
            "</code></pre><script>alert(1)</script>",
        ),
        RichBlock::Image {
            url: "javascript:alert(1)".into(),
            width: None,
            height: None,
        },
    ]);
    let html = d.to_html();
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("javascript"), "{html}");
    assert!(!html.contains("url("), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert!(html.contains("&lt;script&gt;x&lt;/script&gt;"), "{html}");
    assert!(html.contains("language-plaintext"), "{html}");
}

#[test]
fn deeply_nested_markup_does_not_overflow() {
    let html = format!("{}x{}", "<div>".repeat(5000), "</div>".repeat(5000));
    let _ = parse_rich_html(&html);
    let lists = format!("{}x{}", "<ul><li>".repeat(2000), "</li></ul>".repeat(2000));
    let _ = parse_rich_html(&lists).to_html();
}

#[test]
fn encoding_adds_no_mark_but_decoding_drops_one_from_an_older_build() {
    let text = "Abdullah\nامبارح دخلت على الموقع وشفت الحاجة";
    let html = markdown_to_composer_html(text);
    assert!(!html.contains('\u{200F}'), "{html}");
    assert_eq!(
        html,
        "<p dir=\"ltr\">Abdullah</p><p dir=\"rtl\">امبارح دخلت على الموقع وشفت الحاجة</p>"
    );
    // Older builds put a U+200F at the start of such a message.
    let old = html.replacen("<p dir=\"ltr\">", "<p dir=\"ltr\">\u{200F}", 1);
    assert_eq!(
        parse_rich_html(&old),
        doc(vec![p("Abdullah"), p("امبارح دخلت على الموقع وشفت الحاجة")])
    );
}
