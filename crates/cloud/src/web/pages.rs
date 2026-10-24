//! The pages of the account web as plain functions from a few facts to HTML.
//!
//! No template engine and no scripts of our own beyond `passkey.js`: the pages are server
//! rendered, every form works without JavaScript, and everything a visitor controls goes
//! through [`esc`] before it reaches the markup. The look is `assets/style.css`.

use std::fmt::Write as _;

/// What every page needs to know about the cloud and the visitor.
pub struct Ctx<'a> {
    pub cloud_name: &'a str,
    pub csrf: &'a str,
    /// The handle of the signed-in account, shown in the header.
    pub handle: Option<&'a str>,
    pub privacy_url: Option<&'a str>,
    pub terms_url: Option<&'a str>,
    /// A line from the last action, shown once.
    pub notice: Option<&'a str>,
}

/// Escapes text for HTML, in text and in quoted attributes.
pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A Unix time as `2026-10-24` (UTC).
pub fn date(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// A date as a `<time>` element, which the style keeps on one line.
fn time_tag(unix: u64) -> String {
    let day = date(unix);
    format!("<time datetime=\"{day}\">{day}</time>")
}

fn shell(ctx: &Ctx<'_>, title: &str, wide: bool, passkeys: bool, body: &str) -> String {
    let mut html = String::with_capacity(body.len() + 2048);
    html.push_str(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"color-scheme\" content=\"dark light\">\n\
         <meta name=\"robots\" content=\"noindex\">\n",
    );
    let _ = writeln!(html, "<title>{} · AnyPlay</title>", esc(title));
    let _ = writeln!(html, "<meta name=\"csrf\" content=\"{}\">", esc(ctx.csrf));
    html.push_str(
        "<link rel=\"icon\" href=\"/web/assets/icon.svg\" type=\"image/svg+xml\">\n\
         <link rel=\"stylesheet\" href=\"/web/assets/style.css\">\n</head>\n",
    );
    let _ = writeln!(
        html,
        "<body class=\"{}\">",
        if wide { "wide" } else { "narrow" }
    );
    html.push_str(
        "<a class=\"skip\" href=\"#main\">Skip to content</a>\n<header class=\"bar\">\n\
         <a class=\"brand\" href=\"/web/account\"><img src=\"/web/assets/icon.svg\" alt=\"\" width=\"24\" height=\"24\">AnyPlay</a>\n",
    );
    if let Some(handle) = ctx.handle {
        let _ = writeln!(
            html,
            "<div class=\"who\"><span><span class=\"long\">Signed in as </span><strong>{}</strong></span>\
             <form method=\"post\" action=\"/web/sign-out\">{}\
             <button class=\"btn small\" type=\"submit\">Sign out</button></form></div>\n",
            esc(handle),
            csrf_field(ctx)
        );
    }
    let _ = write!(
        html,
        "</header>\n<main id=\"main\"{}>\n",
        if wide { " class=\"wide\"" } else { "" }
    );
    if let Some(notice) = ctx.notice {
        let _ = writeln!(
            html,
            "<p class=\"notice\" role=\"status\">{}</p>",
            esc(notice)
        );
    }
    html.push_str(body);
    html.push_str("\n</main>\n<footer>\n");
    let _ = writeln!(html, "<span>{}</span>", esc(ctx.cloud_name));
    if let Some(url) = ctx.privacy_url {
        let _ = writeln!(html, "<a href=\"{}\">Privacy</a>", esc(url));
    }
    if let Some(url) = ctx.terms_url {
        let _ = writeln!(html, "<a href=\"{}\">Terms</a>", esc(url));
    }
    html.push_str("</footer>\n");
    if passkeys {
        html.push_str("<script src=\"/web/assets/passkey.js\" defer></script>\n");
    }
    html.push_str("</body>\n</html>\n");
    html
}

fn csrf_field(ctx: &Ctx<'_>) -> String {
    format!(
        "<input type=\"hidden\" name=\"csrf\" value=\"{}\">",
        esc(ctx.csrf)
    )
}

/// The message under the control it concerns: what is wrong and how to fix it. The control
/// points at it with `aria-describedby` and takes focus, so a screen reader reads it there.
fn error_line(problem: Option<&str>) -> String {
    match problem {
        Some(text) => format!("<p class=\"error\" id=\"problem\">{}</p>", esc(text)),
        None => String::new(),
    }
}

/// The attributes of an input that has a problem. `described` names other descriptions it keeps.
fn flagged(problem: Option<&str>, described: &str) -> String {
    match (problem.is_some(), described.is_empty()) {
        (true, true) => " aria-invalid=\"true\" aria-describedby=\"problem\" autofocus".to_owned(),
        (true, false) => {
            format!(" aria-invalid=\"true\" aria-describedby=\"problem {described}\" autofocus")
        }
        (false, true) => String::new(),
        (false, false) => format!(" aria-describedby=\"{described}\""),
    }
}

fn device_line(device: Option<&str>) -> String {
    match device {
        Some(name) => format!(
            "<p class=\"lead\">Signing in on <strong>{}</strong>. You will go back to AnyPlay when you are done.</p>",
            esc(name)
        ),
        None => String::new(),
    }
}

// Signing in and creating an account

pub fn sign_in(ctx: &Ctx<'_>, device: Option<&str>, problem: Option<&str>) -> String {
    let mut body = String::new();
    body.push_str("<h1>Sign in</h1>\n");
    if device.is_some() {
        body.push_str(&device_line(device));
    } else {
        body.push_str("<p class=\"lead\">Use your passkey, or get a code by e-mail.</p>\n");
    }
    body.push_str("<div class=\"card\">\n");
    // Without a script, or in a browser that cannot do passkeys, only the e-mail form shows and
    // its button is the primary one. The script reveals the passkey part and demotes the button.
    body.push_str(
        "<div data-needs-passkey hidden>\n\
         <button class=\"btn primary block\" type=\"button\" data-passkey=\"sign-in\" \
         data-status=\"passkey-status\">Sign in with a passkey</button>\n\
         <p class=\"status\" id=\"passkey-status\" role=\"status\"></p>\n\
         <p class=\"or\" aria-hidden=\"true\">or</p>\n</div>\n",
    );
    body.push_str(&email_form(ctx, "sign_in", "Send code", problem, true));
    body.push_str("</div>\n");
    body.push_str(
        "<p class=\"below\">New to AnyPlay? <a class=\"link\" href=\"/web/create\">Create account</a></p>",
    );
    shell(ctx, "Sign in", false, true, &body)
}

pub fn create(ctx: &Ctx<'_>, device: Option<&str>, problem: Option<&str>) -> String {
    let mut body = String::new();
    body.push_str("<h1>Create account</h1>\n");
    if device.is_some() {
        body.push_str(&device_line(device));
    } else {
        body.push_str(
            "<p class=\"lead\">Start with your e-mail address. We send a code to confirm it.</p>\n",
        );
    }
    body.push_str("<div class=\"card\">\n");
    body.push_str(&email_form(ctx, "create", "Send code", problem, true));
    body.push_str("</div>\n");
    body.push_str(
        "<p class=\"below\">Already have an account? <a class=\"link\" href=\"/web/sign-in\">Sign in</a></p>",
    );
    shell(ctx, "Create account", false, false, &body)
}

fn email_form(
    ctx: &Ctx<'_>,
    intent: &str,
    button: &str,
    problem: Option<&str>,
    primary: bool,
) -> String {
    format!(
        "<form method=\"post\" action=\"/web/email\">\n{csrf}\n\
         <input type=\"hidden\" name=\"intent\" value=\"{intent}\">\n\
         <div class=\"field\"><label for=\"email\">E-mail address</label>\n\
         <input id=\"email\" name=\"email\" type=\"email\" autocomplete=\"email\" \
         inputmode=\"email\" placeholder=\"name@example.com\" required{flag}>{error}</div>\n\
         <button class=\"btn{primary}\" type=\"submit\" data-send>{button}</button>\n</form>",
        csrf = csrf_field(ctx),
        flag = flagged(problem, ""),
        error = error_line(problem),
        primary = if primary { " primary" } else { "" },
    )
}

pub fn code(ctx: &Ctx<'_>, email: &str, create: bool, problem: Option<&str>) -> String {
    let body = format!(
        "<h1>Check your e-mail</h1>\n\
         <p class=\"lead\">We sent a 6-digit code to <strong>{email}</strong>. It works for 10 minutes.</p>\n\
         <div class=\"card\">\n\
         <form method=\"post\" action=\"/web/code\">\n{csrf}\n\
         <div class=\"field\"><label for=\"code\">6-digit code</label>\n\
         <input id=\"code\" class=\"code\" name=\"code\" type=\"text\" inputmode=\"numeric\" \
         autocomplete=\"one-time-code\" pattern=\"[0-9]{{6}}\" maxlength=\"6\" placeholder=\"6 digits\" \
         title=\"Enter the 6 digits from the e-mail.\" required{flag}>{error}</div>\n\
         <button class=\"btn primary\" type=\"submit\">Continue</button>\n</form>\n</div>\n\
         <div class=\"actions center top\">\
         <form method=\"post\" action=\"/web/code/resend\">{csrf}\
         <button class=\"btn plain\" type=\"submit\">Send a new code</button></form>\
         <a class=\"btn plain\" href=\"{other}\">Use another address</a></div>",
        email = esc(email),
        error = error_line(problem),
        csrf = csrf_field(ctx),
        flag = flagged(problem, ""),
        other = if create {
            "/web/create"
        } else {
            "/web/sign-in"
        },
    );
    shell(ctx, "Check your e-mail", false, false, &body)
}

/// What the page asks the player to accept.
pub struct TermsView<'a> {
    /// The version in force, shown so the player knows which they accept.
    pub version: &'a str,
}

/// The control of the details form that a problem is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Handle,
    Age,
    Terms,
}

fn terms_link(ctx: &Ctx<'_>) -> String {
    match ctx.terms_url {
        // The player has typed a handle and ticked a box: reading the terms must not lose them.
        Some(url) => format!(
            "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">terms<span class=\"visually-hidden\"> (opens in a new tab)</span></a>",
            esc(url)
        ),
        None => "terms".to_owned(),
    }
}

pub fn details(
    ctx: &Ctx<'_>,
    email: &str,
    terms: Option<&TermsView<'_>>,
    handle: &str,
    problem: Option<(Field, &str)>,
) -> String {
    let about = |field: Field| problem.filter(|(f, _)| *f == field).map(|(_, text)| text);
    let terms_box = match terms {
        Some(view) => format!(
            "<div class=\"field\"><label class=\"check\"><input type=\"checkbox\" name=\"terms\" value=\"yes\" required{flag}>\
             <span>I accept the {link} (version {version}).</span></label>{error}</div>\n",
            link = terms_link(ctx),
            version = esc(view.version),
            flag = flagged(about(Field::Terms), ""),
            error = error_line(about(Field::Terms)),
        ),
        None => String::new(),
    };
    let body = format!(
        "<h1>Choose a handle</h1>\n\
         <p class=\"lead\">Your address <strong>{email}</strong> is confirmed. Other players see your handle.</p>\n\
         <div class=\"card\">\n\
         <form method=\"post\" action=\"/web/details\">\n{csrf}\n\
         <div class=\"field\"><label for=\"handle\">Handle</label>\n\
         <input id=\"handle\" name=\"handle\" type=\"text\" autocomplete=\"nickname\" \
         minlength=\"3\" maxlength=\"16\" pattern=\"[A-Za-z][A-Za-z0-9_\\-]{{2,15}}\" \
         title=\"Use 3 to 16 letters, digits, hyphens or underscores. Start with a letter.\" \
         autocapitalize=\"none\" spellcheck=\"false\" value=\"{handle}\" required{handle_flag}>{handle_error}\n\
         <p class=\"hint\" id=\"handle-hint\">Use 3 to 16 letters, digits, hyphens or underscores. Start with a letter.</p></div>\n\
         <div class=\"field\"><label class=\"check\"><input type=\"checkbox\" name=\"age\" value=\"yes\" required{age_flag}>\
         <span>I am old enough to agree to the terms on my own.<span class=\"hint block-text\">In the EU that is 16, or lower where your country sets it.</span></span></label>{age_error}</div>\n\
         {terms_box}\
         <button class=\"btn primary\" type=\"submit\">Create account</button>\n</form>\n</div>",
        email = esc(email),
        csrf = csrf_field(ctx),
        handle = esc(handle),
        handle_flag = flagged(about(Field::Handle), "handle-hint"),
        handle_error = error_line(about(Field::Handle)),
        age_flag = flagged(about(Field::Age), ""),
        age_error = error_line(about(Field::Age)),
    );
    shell(ctx, "Choose a handle", false, false, &body)
}

pub fn terms(ctx: &Ctx<'_>, view: &TermsView<'_>, problem: Option<&str>) -> String {
    let link = match ctx.terms_url {
        Some(url) => format!(
            "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">Read the terms<span class=\"visually-hidden\"> (opens in a new tab)</span></a>. ",
            esc(url)
        ),
        None => String::new(),
    };
    let body = format!(
        "<h1>Review the terms</h1>\n\
         <p class=\"lead\">The terms changed. {link}Accept version {version} to continue.</p>\n\
         <div class=\"card\">\n\
         <form method=\"post\" action=\"/web/terms\">\n{csrf}\n\
         <div class=\"field\"><label class=\"check\"><input type=\"checkbox\" name=\"accept\" value=\"yes\" required{flag}>\
         <span>I accept the terms (version {version}).</span></label>{error}</div>\n\
         <button class=\"btn primary\" type=\"submit\">Accept and continue</button>\n</form>\n</div>",
        version = esc(view.version),
        csrf = csrf_field(ctx),
        flag = flagged(problem, ""),
        error = error_line(problem),
    );
    shell(ctx, "Review the terms", false, false, &body)
}

/// The page that hands the browser back to the app on this PC.
pub fn back_to_app(ctx: &Ctx<'_>, url: &str) -> String {
    let body = format!(
        "<h1>Signed in</h1>\n\
         <p class=\"lead\">Taking you back to AnyPlay. If nothing happens, use the button.</p>\n\
         <a class=\"btn primary\" href=\"{url}\">Open AnyPlay</a>",
        url = esc(url)
    );
    let mut html = shell(ctx, "Signed in", false, false, &body);
    // A refresh header cannot be sent for a URL that is not ours under every proxy; the markup
    // form works everywhere and needs no script.
    html = html.replacen(
        "<meta name=\"robots\" content=\"noindex\">",
        &format!(
            "<meta name=\"robots\" content=\"noindex\">\n<meta http-equiv=\"refresh\" content=\"0;url={}\">",
            esc(url)
        ),
        1,
    );
    html
}

/// A link from the app: asks before it signs the browser in.
pub fn link(ctx: &Ctx<'_>, handle: &str, code: &str) -> String {
    let body = format!(
        "<h1>Open your account</h1>\n\
         <p class=\"lead\">AnyPlay on your PC asked to open the account of <strong>{handle}</strong> in this browser.</p>\n\
         <div class=\"card\"><form method=\"post\" action=\"/web/link\">{csrf}\n\
         <input type=\"hidden\" name=\"code\" value=\"{code}\">\n\
         <button class=\"btn primary\" type=\"submit\">Open account</button></form></div>\n\
         <p class=\"below\">Did not ask for this? Close the tab.</p>",
        handle = esc(handle),
        csrf = csrf_field(ctx),
        code = esc(code),
    );
    shell(ctx, "Open your account", false, false, &body)
}

pub fn error(ctx: &Ctx<'_>, title: &str, message: &str, action: Option<(&str, &str)>) -> String {
    let button = match action {
        Some((href, label)) => format!(
            "\n<div class=\"actions top\"><a class=\"btn primary\" href=\"{}\">{}</a></div>",
            esc(href),
            esc(label)
        ),
        None => String::new(),
    };
    let body = format!(
        "<h1>{}</h1>\n<p class=\"lead\">{}</p>{button}",
        esc(title),
        esc(message)
    );
    shell(ctx, title, false, false, &body)
}

// The account

pub struct PasskeyView {
    /// The credential ID in URL-safe base64, for the remove form.
    pub id: String,
    pub name: String,
    pub created: u64,
    pub last_used: Option<u64>,
}

pub struct DeviceView {
    pub id: u64,
    pub name: String,
    pub platform: String,
    pub last_used: u64,
}

pub struct AccountView {
    pub handle: String,
    pub email: String,
    pub created: u64,
    pub passkeys: Vec<PasskeyView>,
    pub devices: Vec<DeviceView>,
    pub max_passkeys: usize,
    /// When the account will be deleted, if the player asked for it.
    pub deletion: Option<u64>,
}

fn platform_name(platform: &str) -> &str {
    match platform {
        "windows" => "Windows",
        "linux" => "Linux",
        "macos" => "macOS",
        _ => "Other",
    }
}

pub fn account(ctx: &Ctx<'_>, view: &AccountView) -> String {
    let mut passkeys = String::new();
    if view.passkeys.is_empty() {
        passkeys.push_str(
            "<p class=\"empty\">No passkeys yet. Add one to sign in without waiting for a code.</p>\n",
        );
    } else {
        passkeys.push_str("<ul class=\"rows\" role=\"list\">\n");
        for key in &view.passkeys {
            let used = match key.last_used {
                Some(at) => format!("last used {}", time_tag(at)),
                None => "never used".to_owned(),
            };
            let _ = writeln!(
                passkeys,
                "<li class=\"row\"><div class=\"meta\"><div class=\"name\">{name}</div>\
                 <div class=\"sub\">Added {added}, {used}</div></div>\
                 <form method=\"post\" action=\"/web/account/passkeys/{id}/remove\">{csrf}\
                 <button class=\"btn small danger\" type=\"submit\" aria-label=\"Remove passkey {name_attr}\">Remove</button></form></li>\n",
                name = esc(&key.name),
                name_attr = esc(&key.name),
                added = time_tag(key.created),
                id = esc(&key.id),
                csrf = csrf_field(ctx),
            );
        }
        passkeys.push_str("</ul>\n");
    }
    let add = if view.passkeys.len() >= view.max_passkeys {
        format!(
            "<p class=\"hint\">You have {} passkeys, the most an account can have. Remove one to add another.</p>",
            view.max_passkeys
        )
    } else {
        "<div data-needs-passkey hidden class=\"add\">\n\
         <div class=\"field\"><label for=\"passkey-name\">Passkey name (optional)</label>\n\
         <input id=\"passkey-name\" type=\"text\" maxlength=\"40\" autocomplete=\"off\" \
         placeholder=\"My phone\"></div>\n\
         <button class=\"btn\" type=\"button\" data-passkey=\"add\" data-status=\"add-status\" \
         data-name=\"passkey-name\">Add a passkey</button>\n</div>\n\
         <p class=\"status\" id=\"add-status\" role=\"status\"></p>\n\
         <p class=\"hint\" data-no-passkey hidden>This browser cannot make passkeys. Try another one.</p>"
            .to_owned()
    };

    let mut devices = String::new();
    if view.devices.is_empty() {
        devices.push_str("<p class=\"empty\">No devices are signed in.</p>\n");
    } else {
        devices.push_str("<ul class=\"rows\" role=\"list\">\n");
        for device in &view.devices {
            let _ = writeln!(
                devices,
                "<li class=\"row\"><div class=\"meta\"><div class=\"name\">{name}</div>\
                 <div class=\"sub\">{platform}, last used {used}</div></div>\
                 <form method=\"post\" action=\"/web/account/devices/{id}/sign-out\">{csrf}\
                 <button class=\"btn small\" type=\"submit\" aria-label=\"Sign out {name_attr}\">Sign out</button></form></li>\n",
                name = esc(&device.name),
                name_attr = esc(&device.name),
                platform = platform_name(&device.platform),
                used = time_tag(device.last_used),
                id = device.id,
                csrf = csrf_field(ctx),
            );
        }
        devices.push_str("</ul>\n");
    }

    let pending = match view.deletion {
        Some(at) => format!(
            "<section class=\"section\" aria-labelledby=\"deletion-title\">\n\
             <h2 id=\"deletion-title\">Deletion scheduled</h2>\n\
             <p>This account will be deleted on {at}. Until then you can keep it.</p>\n\
             <form method=\"post\" action=\"/web/account/delete/cancel\">{csrf}\
             <div class=\"actions\"><button class=\"btn\" type=\"submit\">Cancel the deletion</button></div>\
             </form>\n</section>\n",
            at = time_tag(at),
            csrf = csrf_field(ctx),
        ),
        None => String::new(),
    };
    let body = format!(
        "<h1>Your account</h1>\n\
         <dl class=\"facts\"><dt>Handle</dt><dd>{handle}</dd><dt>E-mail</dt><dd>{email}</dd>\
         <dt>Created</dt><dd>{created}</dd></dl>\n{pending}\
         <section class=\"section\" aria-labelledby=\"passkeys-title\">\n\
         <h2 id=\"passkeys-title\">Passkeys</h2>\n\
         <p>A passkey signs you in with your fingerprint, face or screen lock. Your e-mail code still works if you lose it.</p>\n\
         {passkeys}{add}\n</section>\n\
         <section class=\"section\" aria-labelledby=\"devices-title\">\n\
         <h2 id=\"devices-title\">Devices</h2>\n\
         <p>These devices are signed in to your account.</p>\n{devices}\
         <form method=\"post\" action=\"/web/account/sign-out-everywhere\">{csrf}\
         <div class=\"actions\"><button class=\"btn\" type=\"submit\">Sign out everywhere</button></div>\
         <p class=\"hint\">This signs you out everywhere, including this browser.</p></form>\n</section>\n\
         <section class=\"section\" aria-labelledby=\"data-title\">\n\
         <h2 id=\"data-title\">Your data</h2>\n\
         <p>Download everything this cloud stores about you, or delete your account.</p>\n\
         <div class=\"actions\"><a class=\"btn\" href=\"/web/account/export\">Download my data</a>\
         <a class=\"btn danger\" href=\"/web/account/delete\">Delete account</a></div>\n</section>",
        handle = esc(&view.handle),
        email = esc(&view.email),
        created = time_tag(view.created),
        csrf = csrf_field(ctx),
    );
    shell(ctx, "Your account", true, true, &body)
}

/// The appeal part of the ban page.
pub enum AppealPart {
    /// Nothing to say: the ban has no statement the player can appeal.
    None,
    /// An appeal can be filed until `deadline`.
    Form { deadline: u64 },
    /// An appeal was filed; `status` is `open`, `upheld` or `reversed`.
    Filed {
        status: String,
        answer: Option<String>,
    },
    /// The time to appeal has ended.
    Over,
}

/// The statement of reasons of a ban, as the player reads it.
pub struct BanView {
    pub until: Option<u64>,
    pub rule: Option<String>,
    pub reason: Option<String>,
    pub decided: Option<u64>,
    pub appeal: AppealPart,
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub fn banned(ctx: &Ctx<'_>, view: &BanView, problem: Option<&str>) -> String {
    let lead = match view.until {
        Some(until) => format!("The ban lasts until {}.", time_tag(until)),
        None => "The ban is for good.".to_owned(),
    };
    let mut facts = String::new();
    if let Some(rule) = &view.rule {
        let _ = write!(facts, "<dt>Rule</dt><dd>{}</dd>", esc(&capitalised(rule)));
    }
    if let Some(reason) = &view.reason {
        let _ = write!(facts, "<dt>Reason</dt><dd>{}</dd>", esc(reason));
    }
    if let Some(decided) = view.decided {
        let _ = write!(facts, "<dt>Decided</dt><dd>{}</dd>", time_tag(decided));
    }
    let facts = if facts.is_empty() {
        String::new()
    } else {
        format!("<dl class=\"facts\">{facts}</dl>\n")
    };
    let appeal = match &view.appeal {
        AppealPart::None => String::new(),
        AppealPart::Over => "<section class=\"section\"><h2>Appeal</h2><p>The time to appeal has ended.</p></section>".to_owned(),
        AppealPart::Filed { status, answer } => {
            let state = match status.as_str() {
                "upheld" => "A moderator looked at your appeal and the ban stands.",
                "reversed" => "A moderator looked at your appeal and lifted the ban. Sign in again.",
                _ => "A moderator is looking at your appeal.",
            };
            let answer = match answer {
                Some(text) => format!("<p>Their answer: {}</p>", esc(text)),
                None => String::new(),
            };
            format!(
                "<section class=\"section\"><h2>Your appeal</h2><p>{state}</p>{answer}</section>"
            )
        }
        AppealPart::Form { deadline } => format!(
            "<section class=\"section\" aria-labelledby=\"appeal-title\"><h2 id=\"appeal-title\">Appeal</h2>\n\
             <p>If you think this is wrong, tell a moderator why. You can appeal until {deadline}.</p>\n\
             <form method=\"post\" action=\"/web/appeal\">{csrf}\n\
             <div class=\"field\"><label for=\"text\">Why is the ban wrong?</label>\n\
             <textarea id=\"text\" name=\"text\" rows=\"5\" maxlength=\"1000\" required{flag}></textarea>{error}</div>\n\
             <button class=\"btn primary\" type=\"submit\">Send appeal</button></form></section>",
            deadline = time_tag(*deadline),
            csrf = csrf_field(ctx),
            flag = flagged(problem, ""),
            error = error_line(problem),
        ),
    };
    let body = format!(
        "<h1>This account is banned</h1>\n<p class=\"lead\">{lead}</p>\n{facts}{appeal}\n\
         <p class=\"below\"><a class=\"link\" href=\"/web/sign-in\">Back to sign in</a></p>"
    );
    shell(ctx, "This account is banned", false, false, &body)
}

pub fn delete(ctx: &Ctx<'_>, handle: &str, problem: Option<&str>) -> String {
    let body = format!(
        "<h1>Delete your account?</h1>\n\
         <p class=\"lead\">This removes your profile, friends, trophies and devices from this cloud. It cannot be undone.</p>\n\
         <div class=\"card\">\n\
         <form method=\"post\" action=\"/web/account/delete\">\n{csrf}\n\
         <div class=\"field\"><label for=\"confirm\">Type your handle, <strong>{handle}</strong>, to confirm</label>\n\
         <input id=\"confirm\" name=\"handle\" type=\"text\" autocomplete=\"off\" autocapitalize=\"none\" \
         spellcheck=\"false\" required{flag}>{error}</div>\n\
         <div class=\"actions\"><a class=\"btn\" href=\"/web/account\">Keep account</a>\
         <button class=\"btn danger\" type=\"submit\">Delete account</button></div>\n</form>\n</div>",
        handle = esc(handle),
        error = error_line(problem),
        csrf = csrf_field(ctx),
        flag = flagged(problem, ""),
    );
    shell(ctx, "Delete your account", false, false, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx<'static> {
        Ctx {
            cloud_name: "Test <cloud>",
            csrf: "token\"1",
            handle: None,
            privacy_url: Some("https://example.org/privacy"),
            terms_url: None,
            notice: None,
        }
    }

    #[test]
    fn dates_are_iso() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date(1_792_800_000), "2026-10-24");
    }

    #[test]
    fn everything_a_visitor_controls_is_escaped() {
        let evil = "<script>alert(1)</script>\"'&";
        let view = AccountView {
            handle: evil.into(),
            email: evil.into(),
            created: 0,
            passkeys: vec![PasskeyView {
                id: "abc".into(),
                name: evil.into(),
                created: 0,
                last_used: None,
            }],
            devices: vec![DeviceView {
                id: 1,
                name: evil.into(),
                platform: "linux".into(),
                last_used: 0,
            }],
            max_passkeys: 10,
            deletion: Some(0),
        };
        let mut ctx = ctx();
        ctx.handle = Some(evil);
        ctx.notice = Some(evil);
        for html in [
            account(&ctx, &view),
            sign_in(&ctx, Some(evil), Some(evil)),
            create(&ctx, Some(evil), Some(evil)),
            code(&ctx, evil, false, Some(evil)),
            details(
                &ctx,
                evil,
                Some(&TermsView { version: evil }),
                evil,
                Some((Field::Handle, evil)),
            ),
            terms(&ctx, &TermsView { version: evil }, Some(evil)),
            delete(&ctx, evil, Some(evil)),
            link(&ctx, evil, evil),
            back_to_app(&ctx, evil),
            error(&ctx, evil, evil, Some((evil, evil))),
        ] {
            assert!(!html.contains("<script>alert"), "{html}");
            assert!(!html.contains("alert(1)</script>"), "{html}");
        }
    }

    #[test]
    fn every_form_has_a_csrf_token_and_every_input_a_label() {
        let ctx = ctx();
        let html = sign_in(&ctx, None, None);
        assert!(html.contains("name=\"csrf\" value=\"token&quot;1\""));
        assert!(html.contains("<label for=\"email\">"));
        let html = code(&ctx, "a@example.com", false, None);
        assert!(html.contains("<label for=\"code\">6-digit code</label>"));
        assert!(html.contains("autocomplete=\"one-time-code\""));
    }

    #[test]
    fn a_problem_is_announced_and_tied_to_its_field() {
        let html = sign_in(&ctx(), None, Some("That is not an e-mail address."));
        assert!(html.contains("id=\"problem\""));
        assert!(html.contains("aria-invalid=\"true\" aria-describedby=\"problem\""));
        // It sits under the field it is about, not above the form.
        let field = html.find("id=\"email\"").unwrap();
        assert!(html.find("id=\"problem\"").unwrap() > field);
    }

    #[test]
    fn pages_load_nothing_from_other_origins() {
        let ctx = ctx();
        for html in [
            sign_in(&ctx, None, None),
            create(&ctx, None, None),
            code(&ctx, "a@example.com", false, None),
        ] {
            for attribute in ["src=\"", "href=\""] {
                for part in html.split(attribute).skip(1) {
                    let target = part.split('"').next().unwrap();
                    assert!(
                        target.starts_with('/')
                            || target.starts_with('#')
                            || target.starts_with("https://example.org/"),
                        "{target}"
                    );
                }
            }
            assert!(!html.contains("<script>"));
            assert!(
                !html.contains("style=\""),
                "inline styles are blocked by the CSP"
            );
        }
    }
}
