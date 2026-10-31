DNS monitors
============

A ``dns`` monitor asks a DNS server for a record. It answers two different
questions:

* **Does name resolution work?** Without ``expected``, the check succeeds
  when the server returns at least one record of the requested type.
* **Is the record right?** With ``expected``, the answer must contain
  exactly the listed values.

Is the internet down?
---------------------

The install script creates this monitor. It asks a public resolver
(Quad9) for a well-known name:

.. literalinclude:: /examples/catalog/monitors/internet-dns.toml
   :language: toml
   :caption: monitors/internet-dns.toml

Together with a monitor for your router, it tells "the internet connection
is down" (router UP, this DOWN) from "the router is down" (both DOWN).

Your own resolver
-----------------

If your network uses a Pi-hole, AdGuard Home or the router as DNS server,
every device fails when that server fails, even though the internet works.
Ask it directly:

.. literalinclude:: /examples/catalog/monitors/pihole.toml
   :language: toml
   :caption: monitors/pihole.toml

Without ``server``, kinomi asks the first ``nameserver`` from
``/etc/resolv.conf``, i.e. the same server the Pi itself uses.

Your domain points to your server
---------------------------------

After a provider change, a typo in the DNS settings, or a hijacked account,
your domain may point somewhere else. ``expected`` catches that:

.. literalinclude:: /examples/catalog/monitors/own-domain.toml
   :language: toml
   :caption: monitors/own-domain.toml

The answer must be exactly this set of values: one missing or one extra
address fails the check with ``unexpected A records``. For a name with
several addresses, list all of them; the order does not matter. Records
behind a CNAME count, so ``www.example.org`` works even if it is an alias.

Ask the authoritative server of your domain, or a public resolver like
``1.1.1.1``, rather than your local one, which may have an old answer
cached.

Mail records
------------

MX records are written as ``"<preference> <host>"``:

.. literalinclude:: /examples/catalog/monitors/mx.toml
   :language: toml
   :caption: monitors/mx.toml

Names are compared in lowercase and without the trailing dot, so
``Mail.Example.ORG.`` matches ``mail.example.org``.

A TXT record
------------

SPF, DMARC and domain verification records are TXT records. The name may
contain ``_``:

.. literalinclude:: /examples/catalog/monitors/dmarc.toml
   :language: toml
   :caption: monitors/dmarc.toml

A long TXT record that is split into several strings is joined before the
comparison.

Keys
----

.. list-table::
   :header-rows: 1
   :widths: 25 20 55

   * - Key
     - Default
     - Meaning
   * - ``name``
     - (required)
     - The name to look up.
   * - ``record_type``
     - ``"A"``
     - ``A``, ``AAAA``, ``CNAME``, ``MX``, ``TXT`` or ``NS``.
   * - ``server``
     - first ``nameserver`` in ``/etc/resolv.conf``
     - DNS server: ``"1.1.1.1"``, ``"1.1.1.1:53"``, ``"::1"`` or
       ``"[::1]:53"``.
   * - ``expected``
     - none
     - 1-32 values the answer must consist of (any order).

Messages you may see
--------------------

.. list-table::
   :widths: 40 60

   * - ``2 A records``
     - Success: the number of matching records.
   * - ``no MX record``
     - The name exists, but has no record of this type.
   * - ``domain does not exist``
     - The name does not exist (NXDOMAIN).
   * - ``unexpected A records``
     - The answer differs from ``expected``. kinomi never shows the
       received values; use ``dig`` to see them.
   * - ``timeout``
     - The server did not answer.
   * - ``DNS server failure`` / ``DNS query refused``
     - The server answered with an error.
   * - ``DNS response truncated``
     - The answer was too big for one UDP packet. kinomi does not retry
       over TCP; check fewer or shorter records.

kinomi's DNS client is deliberately small: one question over UDP, no cache,
no DNSSEC validation.
