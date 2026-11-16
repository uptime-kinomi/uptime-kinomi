Ping monitors
=============

A ``ping`` monitor sends ICMP echo requests ("pings") to a host and waits
for the answers, like the ``ping`` command. It needs nothing on the other
side but a network stack, so it works for every device in your network,
also one without any open port: a switch, an access point, a phone, a
printer in sleep mode. Available since kinomi 1.2.

A device in your network
------------------------

.. literalinclude:: /examples/catalog/monitors/gateway.toml
   :language: toml
   :caption: monitors/gateway.toml

Every check sends three requests, 500 ms apart. One answer is enough for
the check to succeed; the message says how many came back
(``3/3 replies``), and the latency is their average round trip.

The internet connection
-----------------------

.. literalinclude:: /examples/catalog/monitors/internet-ping.toml
   :language: toml
   :caption: monitors/internet-ping.toml

``loss_warn = true`` turns lost packets into ⚠ WARN (``4/5 replies, packet
loss``). The check still counts as up, and you get a ``warn``
notification (at most once a day). Without ``loss_warn``, only a check
without any reply fails.

Some hosts do not answer pings at all, on purpose (many firewalls drop
them, Windows does by default). For those, use a :doc:`tcp` or
:doc:`http` monitor.

No root needed
--------------

kinomi uses the same kind of socket as the ``ping`` program of current
Linux distributions: an ICMP socket that the kernel allows for ordinary
users, if their group is in the kernel setting
``net.ipv4.ping_group_range``. systemd and Raspberry Pi OS allow it for
everyone, so it usually just works. Check with:

.. code-block:: sh

   cat /proc/sys/net/ipv4/ping_group_range     # "0 2147483647": everyone

If it shows ``1 0`` (nobody), ``check-config`` warns and the check fails
with ``ping not permitted``. An administrator can allow it, now and after
every reboot:

.. code-block:: sh

   echo 'net.ipv4.ping_group_range = 0 2147483647' |
     sudo tee /etc/sysctl.d/60-ping.conf
   sudo sysctl --system

kinomi never needs root or special capabilities for this.

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
     - IP address or host name. A name is looked up at every check; the
       first address is used (IPv6 or IPv4, as the system prefers).
   * - ``count``
     - ``3``
     - Requests per check, 1-10, 500 ms apart. They must fit into
       ``timeout``: 10 requests need more than 4.5 s.
   * - ``loss_warn``
     - ``false``
     - ``true``: WARN when some requests got no reply.
   * - ``slow_warn``
     - ``"0s"`` (off)
     - WARN when the average round trip takes longer than this.

Messages you may see
--------------------

.. list-table::
   :widths: 40 60

   * - ``3/3 replies``
     - All requests were answered (``2/3 replies``: one got lost).
   * - ``2/3 replies, packet loss``
     - Some requests were lost and ``loss_warn`` is on (WARN).
   * - ``3/3 replies, slow response``
     - Slower than ``slow_warn`` (WARN).
   * - ``no reply``
     - No answer within ``timeout``: the host is off, or something drops
       pings.
   * - ``host not found`` / ``host unreachable`` / ``network unreachable``
     - Name or network problems.
   * - ``ping not permitted (net.ipv4.ping_group_range)``
     - The kernel does not allow ping for kinomi's user, see above.
   * - ``IPv6 not available``
     - The host has an IPv6 address, but this computer has no IPv6.
