HTTP(S) monitors
================

An ``http`` monitor requests a URL, like a browser, and checks the answer:
the status code, optionally a keyword on the page, the response time and
the HTTPS certificate.

The simplest one
----------------

.. literalinclude:: /examples/catalog/monitors/website.toml
   :language: toml
   :caption: monitors/website.toml

This is UP when the server answers with a status code from 200 to 299
within 10 seconds, every minute. Everything else (an error code like 500 or
404, no answer, a certificate problem) is a failure.

Make sure the page really works
-------------------------------

A web server can answer ``200 OK`` and still show an empty page, a PHP
error or "Error establishing a database connection". A **keyword** that is
only on the working page catches that:

.. literalinclude:: /examples/catalog/monitors/shop.toml
   :language: toml
   :caption: monitors/shop.toml

Pick a text that is part of the page itself, not of a menu or footer that
an error page might share. The keyword is case-sensitive and compared byte
by byte with the HTML, so ``&amp;`` in the HTML is not ``&``. kinomi reads
at most the first 64 KiB of the page (``max_body_bytes``, up to 1 MiB) and
never stores or shows the page content: the log only says ``keyword
missing``.

The opposite also works: with ``invert_keyword = true``, the check fails
when the text **is** on the page, for example a maintenance notice that
someone forgot to switch off:

.. literalinclude:: /examples/catalog/monitors/maintenance-mode.toml
   :language: toml
   :caption: monitors/maintenance-mode.toml

An API and its response time
----------------------------

Health endpoints of web applications answer quickly and with a predictable
text. This one is checked every 30 seconds, must answer exactly ``200``,
must contain ``"status":"ok"``, and should answer within a second:

.. literalinclude:: /examples/catalog/monitors/api-health.toml
   :language: toml
   :caption: monitors/api-health.toml

* ``expected_status`` is a list of codes and ranges as text, e.g. ``[200,
  204]`` or ``["200-299", 401]``. The default is ``["200-299"]``.
* TOML strings in single quotes take everything literally, so the double
  quotes in the keyword need no escaping.
* ``headers`` adds request headers (kinomi itself sends ``User-Agent:
  uptime-kinomi/<version>``).
* ``slow_warn`` turns an answer slower than one second into ⚠ WARN. It
  still counts as up, and you get a ``warn`` notification (at most once a
  day). It must be shorter than ``timeout``.

The certificate
---------------

kinomi reads the expiry date of the HTTPS certificate and shows it on the
pages. Let it warn you in time:

.. literalinclude:: /examples/catalog/monitors/certificate.toml
   :language: toml
   :caption: monitors/certificate.toml

* ``tls_expiry_warn = "14d"``: ⚠ WARN, and a notification, 14 days before
  the certificate expires. Let's Encrypt certificates are renewed 30 days
  before they expire, so a warning means the renewal has failed for two
  weeks.
* ``tls_expiry_down = "3d"``: the check fails 3 days before. "0s" (the
  default for both) switches a limit off.
* ``method = "HEAD"`` asks only for the headers, not the page: less traffic
  for a check that does not need the content. A keyword needs ``GET``.

To keep checks cheap on slow hardware, kinomi reads the certificate in an extra connection at
most every 6 hours. The ``tls_*`` keys can also be added to any other
``https://`` monitor, like the website above; a separate monitor is only
useful if you want a different interval or name for it.

A certificate that is already invalid (expired, for another name, from an
unknown issuer) makes every check fail, with or without these keys, e.g.
``TLS certificate expired``.

Redirects
---------

By default kinomi does not follow redirects: a ``301`` or ``302`` is the
answer, and it is not in ``200-299``, so the check fails. That is on
purpose: you see that the URL redirects and can put the final URL into the
monitor. To check that a redirect works, follow it:

.. literalinclude:: /examples/catalog/monitors/old-domain.toml
   :language: toml
   :caption: monitors/old-domain.toml

A redirect from ``https://`` to ``http://`` always fails the check. Basic
Auth credentials and custom headers are only sent to the original server,
never to another one that a redirect points to.

A router or NAS in your network
-------------------------------

Devices in your network often have a web interface with a self-signed
certificate, which a normal check rejects (``TLS certificate from unknown
issuer``). For those, switch the certificate check off:

.. literalinclude:: /examples/catalog/monitors/router.toml
   :language: toml
   :caption: monitors/router.toml

``check-config`` warns about ``tls_verify = false``, as a reminder that
anyone in between could pretend to be the device. ``expected_status =
["200-399"]`` accepts the redirect to a login page that many devices send.

A page with a password
----------------------

For a page behind HTTP Basic Auth, give kinomi a user and password. Keep the
password in a file only you can read:

.. code-block:: sh

   echo 'the password' > ~/.config/uptime-kinomi/secrets/intranet.password
   chmod 600 ~/.config/uptime-kinomi/secrets/intranet.password

.. literalinclude:: /examples/catalog/monitors/intranet.toml
   :language: toml
   :caption: monitors/intranet.toml

``basic_auth_password = "..."`` in the file works too, but then the monitor
file itself is a secret (kinomi warns if others can read it). For other
kinds of login, such as an API token, use ``headers``, e.g. ``headers = {
"Authorization" = "Bearer ..." }``.

All keys
--------

.. list-table::
   :header-rows: 1
   :widths: 25 20 55

   * - Key
     - Default
     - Meaning
   * - ``url``
     - (required)
     - ``http://`` or ``https://`` URL, at most 2048 characters, without
       user name and password in it.
   * - ``method``
     - ``"GET"``
     - ``"GET"`` or ``"HEAD"``. A keyword needs ``GET``.
   * - ``expected_status``
     - ``["200-299"]``
     - Accepted status codes: numbers and ranges like ``"200-399"``.
   * - ``keyword``
     - none
     - Text (1-256 bytes) that must be in the first ``max_body_bytes`` of
       the page. Case-sensitive.
   * - ``invert_keyword``
     - ``false``
     - ``true``: the keyword must **not** be on the page.
   * - ``max_body_bytes``
     - ``65536``
     - How much of the page to search, at most 1 MiB.
   * - ``max_redirects``
     - ``0``
     - How many redirects to follow, at most 10.
   * - ``headers``
     - none
     - Extra request headers, e.g. ``{ "Accept" = "application/json" }``.
   * - ``basic_auth_user``
     - none
     - User for HTTP Basic Auth.
   * - ``basic_auth_password`` / ``basic_auth_password_file``
     - none
     - Its password, inline or from a file.
   * - ``tls_verify``
     - ``true``
     - ``false`` accepts any certificate (warns).
   * - ``tls_expiry_warn``
     - ``"0s"`` (off)
     - WARN when the certificate expires within this time.
   * - ``tls_expiry_down``
     - ``"0s"`` (off)
     - Fail when the certificate expires within this time.
   * - ``slow_warn``
     - ``"0s"`` (off)
     - WARN when the answer takes longer than this.

Messages you may see
--------------------

These appear in the log and on private pages:

.. list-table::
   :widths: 40 60

   * - ``HTTP 200``
     - Success (with the status code).
   * - ``HTTP 503``
     - The status code is not in ``expected_status``.
   * - ``HTTP 200, keyword missing`` / ``keyword present``
     - The keyword check failed.
   * - ``HTTP 200, slow response``
     - Slower than ``slow_warn`` (WARN).
   * - ``HTTP 200, certificate expires in 10 days``
     - Within ``tls_expiry_warn`` (WARN) or ``tls_expiry_down`` (failure).
   * - ``timeout``
     - No complete answer within ``timeout``.
   * - ``connection refused``
     - Nothing listens on that port; the server or service is down.
   * - ``host not found``
     - The name does not resolve. A typo, or DNS is broken.
   * - ``TLS certificate expired``, ``... from unknown issuer``, ``... not
       valid for this host name``
     - Certificate problems. Self-signed? See the router example.
   * - ``redirect from https to http``
     - The server redirected to an unencrypted URL.
