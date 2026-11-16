TCP monitors
============

A ``tcp`` monitor opens a connection to a host and port. If the connection
is accepted, the check succeeds. That works for almost every network
service: SSH, databases, mail servers, MQTT, printers, game servers.

Optionally, kinomi also reads the first bytes the server sends (its
*banner*) and compares their beginning with a text. Many protocols start
with a greeting, and checking it proves that the right program answers,
not just anything that accepts connections.

SSH on a NAS
------------

.. literalinclude:: /examples/catalog/monitors/nas-ssh.toml
   :language: toml
   :caption: monitors/nas-ssh.toml

An SSH server greets every client with ``SSH-2.0-...``. If the NAS hangs
in a way that the port still accepts connections but the SSH server does
not answer, the check fails with ``timeout waiting for banner``.

``description`` is shown on private pages only, a good place for a hint
to whoever looks at the page.

A database
----------

Databases (PostgreSQL, MySQL, Redis, ...) wait for the client to speak
first, so they send no banner. An accepted connection is all kinomi can
check without logging in:

.. literalinclude:: /examples/catalog/monitors/database.toml
   :language: toml
   :caption: monitors/database.toml

A mail server
-------------

SMTP servers greet with ``220`` when they are ready to accept mail. A
server that is overloaded or misconfigured often greets with ``421`` or
``554`` instead, which this check catches:

.. literalinclude:: /examples/catalog/monitors/mail-server.toml
   :language: toml
   :caption: monitors/mail-server.toml

The same works for other text protocols: FTP (``220``), POP3 (``+OK``),
IMAP (``* OK``).

A port with TLS
---------------

Many services speak TLS from the first byte: IMAPS (993), SMTPS (465),
LDAPS (636), MQTT over TLS (8883). (Not PostgreSQL or MySQL: they start
TLS inside their own protocol; check those without ``tls``.) With ``tls = true``,
kinomi does the TLS handshake after connecting, checks the certificate
like an :doc:`http` monitor does, and warns before it expires (since
kinomi 1.2):

.. literalinclude:: /examples/catalog/monitors/imaps.toml
   :language: toml
   :caption: monitors/imaps.toml

The banner is then read through TLS. For a device in your network with a
self-signed certificate, set ``tls_verify = false`` (``check-config`` warns
about it, on purpose); the expiry warning still works.

Ports where the server first talks without TLS and switches later
(STARTTLS: SMTP on 25 or 587, IMAP on 143) are not supported; check those
without ``tls``.

A service on the same Pi
------------------------

kinomi can watch its neighbours. ``127.0.0.1`` is the Pi itself:

.. literalinclude:: /examples/catalog/monitors/mqtt.toml
   :language: toml
   :caption: monitors/mqtt.toml

Keys
----

.. list-table::
   :header-rows: 1
   :widths: 25 20 55

   * - Key
     - Default
     - Meaning
   * - ``host``
     - (required)
     - IP address or host name.
   * - ``port``
     - (required)
     - Port number, 1-65535.
   * - ``expected_banner``
     - none
     - Text the server's first bytes must start with (1-256 bytes).
   * - ``tls``
     - ``false``
     - ``true``: TLS handshake after connecting (implicit TLS).
   * - ``tls_verify``
     - ``true``
     - ``false`` accepts any certificate (warns).
   * - ``tls_expiry_warn``
     - ``"14d"``
     - WARN when the certificate expires within this time.
   * - ``tls_expiry_fail``
     - ``"0s"`` (off)
     - Fail when the certificate expires within this time.

All ``tls_*`` keys need ``tls = true``.

Messages you may see
--------------------

.. list-table::
   :widths: 40 60

   * - ``connected``
     - The connection was accepted (no banner check).
   * - ``TLS ok``
     - The connection was accepted and the TLS handshake succeeded.
   * - ``TLS ok, certificate expires in 10 days``
     - Within ``tls_expiry_warn`` (WARN) or ``tls_expiry_fail`` (failure).
       With ``expected_banner``: ``banner ok, certificate expires in 10
       days``.
   * - ``TLS ok, certificate check failed``
     - The handshake worked, but the certificate's expiry could not be
       read (WARN).
   * - ``TLS certificate expired``, ``... from unknown issuer``, ``... not
       valid for this host name``, ``TLS handshake failed``
     - Certificate problems, or the port does not speak TLS.
   * - ``banner ok``
     - The connection was accepted and the banner matched.
   * - ``unexpected banner``
     - The server sent something else. kinomi never shows what it was.
   * - ``timeout waiting for banner`` / ``connection closed before banner``
     - The server accepted the connection but did not greet.
   * - ``connection refused``
     - The host is up, but nothing listens on the port.
   * - ``timeout``
     - No answer at all: the host is off, or a firewall drops the
       connection.
   * - ``host not found`` / ``host unreachable`` / ``network unreachable``
     - Name or network problems.
