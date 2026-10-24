// Passkeys in the browser: the server makes the options and checks the answers (webauthn-rs);
// this file only carries them to and from navigator.credentials. No framework, no libraries.
"use strict";

(function () {
  var csrf = (document.querySelector('meta[name="csrf"]') || {}).content || "";
  var supported = !!(window.PublicKeyCredential && navigator.credentials);

  function toBuffer(text) {
    var base64 = text.replace(/-/g, "+").replace(/_/g, "/");
    while (base64.length % 4) base64 += "=";
    var raw = atob(base64);
    var bytes = new Uint8Array(raw.length);
    for (var i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
    return bytes.buffer;
  }

  function toText(buffer) {
    var bytes = new Uint8Array(buffer);
    var raw = "";
    for (var i = 0; i < bytes.length; i++) raw += String.fromCharCode(bytes[i]);
    return btoa(raw).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }

  function post(path, body) {
    return fetch(path, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
      body: JSON.stringify(body || {}),
    }).then(function (response) {
      return response.json().then(
        function (json) {
          if (!response.ok) throw new Error(json.message || "Something went wrong. Try again.");
          return json;
        },
        function () {
          throw new Error("Something went wrong. Try again.");
        }
      );
    });
  }

  function describe(error) {
    if (error && error.name === "NotAllowedError") {
      return "The request was cancelled or timed out. Try again.";
    }
    if (error && error.name === "InvalidStateError") {
      return "This passkey is already on your account.";
    }
    if (error && error.name === "SecurityError") {
      return "This browser does not allow passkeys on this address.";
    }
    return (error && error.message) || "Something went wrong. Try again.";
  }

  function creationOptions(options) {
    var key = options.publicKey;
    key.challenge = toBuffer(key.challenge);
    key.user.id = toBuffer(key.user.id);
    (key.excludeCredentials || []).forEach(function (c) {
      c.id = toBuffer(c.id);
    });
    return options;
  }

  function requestOptions(options) {
    var key = options.publicKey;
    key.challenge = toBuffer(key.challenge);
    (key.allowCredentials || []).forEach(function (c) {
      c.id = toBuffer(c.id);
    });
    // The server asks for conditional mediation so that browsers may offer passkeys in the
    // autofill list. A button asks for the dialog instead.
    delete options.mediation;
    return options;
  }

  function registrationJson(credential) {
    var response = credential.response;
    return {
      id: credential.id,
      rawId: toText(credential.rawId),
      type: credential.type,
      extensions: credential.getClientExtensionResults ? credential.getClientExtensionResults() : {},
      response: {
        attestationObject: toText(response.attestationObject),
        clientDataJSON: toText(response.clientDataJSON),
        transports: response.getTransports ? response.getTransports() : [],
      },
    };
  }

  function assertionJson(credential) {
    var response = credential.response;
    return {
      id: credential.id,
      rawId: toText(credential.rawId),
      type: credential.type,
      extensions: credential.getClientExtensionResults ? credential.getClientExtensionResults() : {},
      response: {
        authenticatorData: toText(response.authenticatorData),
        clientDataJSON: toText(response.clientDataJSON),
        signature: toText(response.signature),
        userHandle: response.userHandle ? toText(response.userHandle) : null,
      },
    };
  }

  function busy(button, status, on, label) {
    // Not "disabled": a disabled button drops keyboard focus, and the player loses their place.
    button.setAttribute("aria-disabled", on ? "true" : "false");
    if (label) button.textContent = on ? label.busy : label.idle;
    if (on) {
      status.className = "status";
      status.textContent = "";
    }
  }

  function fail(status, error) {
    status.className = "status error";
    status.textContent = describe(error);
  }

  function signIn(button) {
    var status = document.getElementById(button.getAttribute("data-status"));
    var label = { idle: button.textContent, busy: "Waiting for your device…" };
    button.addEventListener("click", function () {
      if (button.getAttribute("aria-disabled") === "true") return;
      busy(button, status, true, label);
      post("/web/passkey/sign-in/start")
        .then(function (started) {
          return navigator.credentials
            .get(requestOptions(started.options))
            .then(function (credential) {
              if (!credential) throw new Error("No passkey was chosen.");
              return post("/web/passkey/sign-in/finish", {
                id: started.id,
                credential: assertionJson(credential),
              });
            });
        })
        .then(function (done) {
          window.location.assign(done.next);
        })
        .catch(function (error) {
          busy(button, status, false, label);
          fail(status, error);
        });
    });
  }

  function add(button) {
    var status = document.getElementById(button.getAttribute("data-status"));
    var name = document.getElementById(button.getAttribute("data-name"));
    var label = { idle: button.textContent, busy: "Waiting for your device…" };
    button.addEventListener("click", function () {
      if (button.getAttribute("aria-disabled") === "true") return;
      busy(button, status, true, label);
      var chosen = name ? name.value.trim() : "";
      post("/web/account/passkeys/start", { name: chosen })
        .then(function (started) {
          return navigator.credentials
            .create(creationOptions(started.options))
            .then(function (credential) {
              if (!credential) throw new Error("No passkey was made.");
              return post("/web/account/passkeys/finish", {
                id: started.id,
                name: chosen,
                credential: registrationJson(credential),
              });
            });
        })
        .then(function () {
          window.location.assign("/web/account");
        })
        .catch(function (error) {
          busy(button, status, false, label);
          fail(status, error);
        });
    });
  }

  // Without passkey support the page stays as the server made it: e-mail only, and a note on the
  // account page. With it, the passkey parts appear, and on the sign-in page the passkey button
  // becomes the main action.
  Array.prototype.forEach.call(document.querySelectorAll("[data-needs-passkey]"), function (node) {
    node.hidden = !supported;
  });
  Array.prototype.forEach.call(document.querySelectorAll("[data-no-passkey]"), function (node) {
    node.hidden = supported;
  });
  if (supported) {
    Array.prototype.forEach.call(document.querySelectorAll("[data-send]"), function (button) {
      button.classList.remove("primary");
    });
    Array.prototype.forEach.call(document.querySelectorAll("[data-passkey]"), function (button) {
      if (button.getAttribute("data-passkey") === "sign-in") signIn(button);
      else add(button);
    });
  }
})();
