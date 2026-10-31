Notifications
=============

A *notifier* is a channel kinomi sends messages to: an app on your phone,
a chat, or any program that accepts a web request. Each notifier is one file
in ``notifications/``; the file name is its ID. Monitors list the notifiers
they use:

.. code-block:: toml
   :caption: monitors/website.toml

   notifications = ["phone", "team-chat"]

or, for all monitors at once, in ``kinomi.toml``:

.. code-block:: toml
   :caption: kinomi.toml

   [defaults]
   notifications = ["phone"]

Which one should I use?
-----------------------

.. list-table::
   :header-rows: 1
   :widths: 25 50 25

   * - Channel
     - Good for
     - Page
   * - ntfy
     - Push messages to your phone. Free, no account; can be self-hosted.
     - :doc:`ntfy`
   * - Gotify
     - Push messages from your own server to your phone (Android).
     - :doc:`gotify`
   * - Telegram
     - Messages in Telegram, to you or a group.
     - :doc:`telegram`
   * - Discord, Slack, Mattermost, Rocket.Chat
     - Messages in a team or family chat.
     - :doc:`webhook`
   * - Webhook (JSON)
     - Home Assistant, Node-RED, n8n, your own script.
     - :doc:`webhook`

.. toctree::
   :maxdepth: 1

   ntfy
   gotify
   telegram
   webhook

What you get notified about
---------------------------

.. list-table::
   :header-rows: 1
   :widths: 15 40 45

   * - Event
     - When
     - Example
   * - ``down``
     - A monitor goes DOWN.
     - ``DOWN: My blog (HTTP 503) at 2026-10-22 16:03 CEST``
   * - ``up``
     - It is no longer DOWN.
     - ``UP: My blog (HTTP 200) after 12 min down at 2026-10-22 16:15 CEST``
   * - ``warn``
     - It gets a warning, e.g. an expiring certificate. At most once a day
       per monitor.
     - ``WARN: My blog (HTTP 200, certificate expires in 10 days) at ...``
   * - ``reminder``
     - It is still DOWN after the monitor's ``resend_interval``.
     - ``STILL DOWN: My blog (timeout) for 6 h at ...``

Times are in the time zone of ``kinomi.toml`` (``[pages] timezone``). The
text only contains kinomi's own summary, never anything the checked server
sent.

To receive only some events on a channel, list them in the notifier file:

.. code-block:: toml

   events = ["down", "up"]        # default: all four

For example: everything to your phone, but only ``down`` and ``up`` to the
family chat.

There are no notifications during maintenance windows (except ``up`` from
a push with ``status=up``), and a monitor that
was already DOWN before a restart does not notify again.

Did it arrive?
--------------

Every notification is logged with its result:

.. code-block:: text

   2026-10-22T14:05:11Z event=NOTIFY monitor=blog notifier=phone kind=down ok=true
   2026-10-22T14:05:41Z event=NOTIFY monitor=blog notifier=chat kind=down ok=false msg="HTTP 401"

kinomi tries each notification up to three times (after 5 and 30 seconds).
``ok=false`` after the last attempt means it was not delivered; ``msg``
says why:

.. list-table::
   :widths: 30 70

   * - ``HTTP 401``, ``HTTP 403``
     - The token or webhook URL is wrong or was revoked.
   * - ``HTTP 404``
     - Wrong URL, or the webhook was deleted.
   * - ``HTTP 301`` and similar
     - The URL redirects. kinomi does not follow redirects for
       notifications; use the final URL.
   * - ``timeout``, ``host not found``
     - The service is not reachable from the Pi.
   * - ``TLS certificate ...``
     - A certificate problem of the service.

To find all failed notifications:

.. code-block:: sh

   grep -h 'event=NOTIFY.*ok=false' ~/.local/state/uptime-kinomi/log/*.log

Test a new channel
------------------

The tutorial shows how: add a monitor that is sure to fail, with
``retries = 0`` and a short interval, and remove it afterwards
(:ref:`first-steps-test-alarm`).

Common keys
-----------

.. code-block:: toml

   config_version = 1
   type = "ntfy"                  # ntfy | gotify | telegram | webhook
   events = ["down", "up", "warn", "reminder"]

Then the section of the type (``[ntfy]``, ``[gotify]``, ...), described on
the pages above. Their URLs may be ``http://`` (e.g. a server in your LAN)
or ``https://``, never with a user name and password in the URL.
