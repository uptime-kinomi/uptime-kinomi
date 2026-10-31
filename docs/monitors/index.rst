Monitors
========

A monitor is one thing kinomi checks: a website, a port, a DNS record, or a
job that reports in. Each monitor is one file in ``monitors/``; the file name
is its ID.

Which type do I need?
---------------------

.. list-table::
   :header-rows: 1
   :widths: 40 15 45

   * - I want to know whether ...
     - Type
     - Page
   * - a website or web API works, shows the right content, or has a valid
       certificate
     - ``http``
     - :doc:`http`
   * - a device or service accepts connections: SSH, a database, a mail
       server, a router, a printer
     - ``tcp``
     - :doc:`tcp`
   * - name resolution works, or a DNS record has the right value
     - ``dns``
     - :doc:`dns`
   * - a backup, cron job or script ran (it calls kinomi), or another
       kinomi is alive
     - ``push``
     - :doc:`push`

``http``, ``tcp`` and ``dns`` are *active*: kinomi performs the check.
``push`` is *passive*: something else calls kinomi, and kinomi notices when
the call does not come.

.. toctree::
   :maxdepth: 1

   http
   tcp
   dns
   push

.. _common-keys:

Keys for every monitor
----------------------

These keys work in every monitor file, before the type's own section
(``[http]``, ``[tcp]``, ...):

.. code-block:: toml

   config_version = 1
   type = "http"                  # http | tcp | dns | push
   name = "My blog"               # shown on pages and in notifications, 1-100 characters
   description = "..."            # optional, shown on private pages only
   interval = "60s"               # how often to check; at least 10s
   timeout = "10s"                # give up after this; less than interval
   retries = 1                    # failures tolerated before DOWN (0-10)
   notifications = ["phone"]      # notifier IDs (files in notifications/)
   resend_interval = "0s"         # repeat the DOWN notification while down; 0s = never
   enabled = true                 # false: no checks, shown as PAUSED
   tags = ["web", "family"]       # optional labels, for pages and maintenance windows

Only ``config_version``, ``type``, ``name`` and the type's section are
required. Defaults for ``interval``, ``timeout``, ``retries`` and
``notifications`` can be set for all monitors at once in the ``[defaults]``
section of ``kinomi.toml``:

.. code-block:: toml
   :caption: kinomi.toml

   [defaults]
   interval = "60s"
   timeout = "10s"
   retries = 1
   notifications = ["phone"]

A value in a monitor file always wins over ``[defaults]``.

Choosing interval, timeout and retries
--------------------------------------

``interval``
   How quickly do you need to know? Every minute is a good default for
   websites. Things that change slowly (certificates, DNS records, mail
   records) are fine with ``"1h"`` or even ``"1d"``. A Pi 1 easily handles
   50 monitors at one minute.

``timeout``
   How long a check may take before it counts as failed. 10 seconds is
   generous for most things; use less for services in your LAN, where an
   answer takes milliseconds.

``retries``
   How many failures in a row to tolerate before DOWN (see
   :doc:`/getting-started/concepts`). With ``interval = "60s"`` and
   ``retries = 1`` you hear about an outage after one to two minutes, and a
   single dropped packet does not wake you up at night.

``resend_interval``
   A reminder while a monitor stays DOWN, e.g. ``"6h"``. Useful for things
   that are easy to forget, such as a backup that keeps failing.

Pausing a monitor
-----------------

To stop checking something for a while without deleting its file and
history, set ``enabled = false`` and restart. It stays on the pages as
‖ PAUSED and counts in no uptime figure. For planned work at a known time,
a maintenance window is better (:doc:`/guides/incidents-and-maintenance`).

Removing a monitor
------------------

Delete its file and restart. Its log lines stay in the log until they
expire. If you add a monitor with the same ID later, it continues with the
old history.
