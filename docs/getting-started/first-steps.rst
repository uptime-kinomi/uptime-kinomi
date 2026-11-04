First steps
===========

This tutorial turns the starter installation into something useful. At the
end you have:

* a monitor for your own website and one for your router,
* notifications on your phone, tested,
* a private page with a password that shows all details,
* a check that your nightly backup really ran.

It takes about 30 minutes. You need a kinomi installed with
:doc:`/installation/install-script` and ``--listen 0.0.0.0:8080``, so you
can open the status page from your laptop. Everything happens in a terminal
on the Pi, as the user that runs kinomi.

.. contents:: Steps
   :local:
   :depth: 1

The one thing to know first
---------------------------

kinomi reads its configuration **once, at startup**. After every change you
do the same two things: let kinomi check the files, then restart it.

.. code-block:: sh

   uptime-kinomi check-config && systemctl --user restart uptime-kinomi

If ``check-config`` finds a problem, it tells you the file and the line,
and the ``&&`` makes sure the running kinomi is not restarted with a broken
configuration. Keep this line at hand; you need it after every step.

All configuration files are in one directory. Go there:

.. code-block:: sh

   cd ~/.config/uptime-kinomi
   ls

.. code-block:: text

   README.txt  kinomi.toml  monitors  notifications  pages

1. Your own website
-------------------

Each monitor is one file in ``monitors/``. The file name (without
``.toml``) is the monitor's ID; use lowercase letters, digits, ``-`` and
``_``. Create ``monitors/website.toml`` with ``nano monitors/website.toml``:

.. literalinclude:: /examples/tutorial/monitors/website.toml
   :language: toml
   :caption: monitors/website.toml

Line by line:

``config_version = 1``
   Every kinomi file starts with this line. It lets future versions
   recognise old files.

``type = "http"``
   An HTTP(S) check: kinomi requests the page like a browser would.

``name = "Our website"``
   The name shown on the status page and in notifications.

``url``
   The address to check. Use your own website here.

``keyword = "Welcome"``
   A word that must be on the page. Replace it with something that is on
   *your* page when it works, e.g. a heading. This catches a site that
   answers but shows an error. Leave the line out if you don't need it.

``tls_expiry_warn = "14d"``
   Warn 14 days before the HTTPS certificate expires.

Everything else has sensible defaults: check every 60 seconds, give up
after 10 seconds, and report DOWN after two failed checks in a row.

You no longer need the example monitor from the installation:

.. code-block:: sh

   rm monitors/example-web.toml

Now check and restart:

.. code-block:: sh

   uptime-kinomi check-config && systemctl --user restart uptime-kinomi

``check-config`` lists your monitors:

.. code-block:: text

   monitors:   2
     internet-dns             dns   every    5m  example.org A @9.9.9.9:53
     website                  http  every    1m  https://www.example.org/
   ...
   configuration OK

Open the status page (``http://raspberrypi.local:8080/``) and wait a
minute. Your website appears as ✓ **UP**. kinomi also writes every check
into its log, one file per day:

.. code-block:: sh

   tail ~/.local/state/uptime-kinomi/log/$(date -u +%F).log

.. code-block:: text

   2026-11-01T10:12:20Z event=START version=1.1.0 monitors=2
   2026-11-01T10:12:20Z monitor=internet-dns result=UP latency_ms=38 msg="2 A records"
   2026-11-01T10:12:27Z monitor=website result=UP latency_ms=120 cert_expires=2026-12-25 msg="HTTP 200"

If you see ``result=FAIL ... msg="HTTP 200, keyword missing"`` instead, the
page loads but does not contain your keyword. Check the spelling: upper and
lower case matter.

.. note::

   Typos in key names are errors, not ignored. If you write ``keywrod``,
   ``check-config`` says so instead of silently checking without a keyword.

2. Your router
--------------

Not everything is a website. A TCP monitor only checks that a device
accepts connections on a port. Most routers have a web interface on port
80. Create ``monitors/router.toml``:

.. literalinclude:: /examples/tutorial/monitors/router.toml
   :language: toml
   :caption: monitors/router.toml

Use your router's address (``ip route | grep default`` shows it). Check and
restart as before:

.. code-block:: sh

   uptime-kinomi check-config && systemctl --user restart uptime-kinomi

Together with the ``internet-dns`` monitor from the installation, your page
now answers the family's favourite question: when the router is UP but
*Internet (DNS)* is DOWN, the problem is the internet connection, not the
Wi-Fi.

3. Notifications on your phone
------------------------------

A status page only helps when someone looks at it. Let kinomi tell you
when something breaks. This tutorial uses `ntfy <https://ntfy.sh>`_: a free
app for Android and iOS, with no account needed.

#. Install the **ntfy** app on your phone.
#. Think of a **topic name**: anyone who knows it can read your messages,
   so make it long and hard to guess, like ``kinomi-7f3a9c2e81d4b6``. A
   quick way to get one on the Pi: ``echo kinomi-$(head -c 8 /dev/urandom |
   od -An -tx1 | tr -d ' \n')``.
#. In the app, tap **+** and subscribe to that topic on ``ntfy.sh``.

Each notification channel is one file in ``notifications/``. Create
``notifications/phone.toml`` with your topic:

.. literalinclude:: /examples/tutorial/notifications/phone.toml
   :language: toml
   :caption: notifications/phone.toml

Now tell kinomi which monitors should use it. You could add
``notifications = ["phone"]`` to every monitor file, but it is easier to
make it the default for all of them. Add this to the end of
``kinomi.toml``:

.. code-block:: toml
   :caption: kinomi.toml (added at the end)

   [defaults]
   notifications = ["phone"]

Check and restart:

.. code-block:: sh

   uptime-kinomi check-config && systemctl --user restart uptime-kinomi

You get a notification when a monitor goes DOWN, when it is UP again, and
when there is a warning such as an expiring certificate.

.. _first-steps-test-alarm:

4. Break something on purpose
-----------------------------

Don't trust notifications you have never seen. Create a monitor that is
guaranteed to fail: it checks port 9 on the Pi itself, where nothing
listens. ``monitors/test-alarm.toml``:

.. code-block:: toml
   :caption: monitors/test-alarm.toml

   config_version = 1
   type = "tcp"
   name = "Test alarm"
   interval = "10s"
   timeout = "2s"
   retries = 0              # DOWN at the first failure

   [tcp]
   host = "127.0.0.1"
   port = 9

.. code-block:: sh

   uptime-kinomi check-config && systemctl --user restart uptime-kinomi

Within about 15 seconds your phone shows **Test alarm is DOWN**, and the
log says that the notification was delivered:

.. code-block:: text

   2026-11-01T10:30:25Z monitor=test-alarm result=DOWN latency_ms=0 msg="connection refused"
   2026-11-01T10:30:25Z event=NOTIFY monitor=test-alarm notifier=phone kind=down ok=true

``ok=false`` means that kinomi could not deliver it: the ``msg`` tells you
why, e.g. no network or a wrong token.

Works? Remove the test monitor and restart:

.. code-block:: sh

   rm monitors/test-alarm.toml
   uptime-kinomi check-config && systemctl --user restart uptime-kinomi

5. A private page with a password
---------------------------------

The page so far is public: anyone who can reach the Pi can see it. It shows
names and states, but no addresses or error messages. A **private** page
shows everything, including what exactly failed, and kinomi's own health.
It needs a user name and a password.

Create a password for the user ``admin``:

.. code-block:: sh

   uptime-kinomi hash-password admin >> htpasswd
   chmod 600 htpasswd

kinomi generates a random password and shows it **once**:

.. code-block:: text

   password for admin: suJFqknfX687_vegLw2w

Write it down or put it into your password manager. The file ``htpasswd``
only contains a hash of it. (To choose your own password instead: ``echo
'my password' | uptime-kinomi hash-password admin >> htpasswd``.)

Tell kinomi about the file: add ``htpasswd_file`` to the ``[server]``
section of ``kinomi.toml``:

.. code-block:: toml
   :caption: kinomi.toml
   :emphasize-lines: 4

   [server]
   listen = "0.0.0.0:8080"
   hostnames = ["localhost", "127.0.0.1", "raspberrypi", "raspberrypi.local", "192.168.1.50"]
   htpasswd_file = "htpasswd"

And create the page, ``pages/all.toml``. ``"*"`` means all monitors:

.. literalinclude:: /examples/tutorial/pages/all.toml
   :language: toml
   :caption: pages/all.toml

.. code-block:: sh

   uptime-kinomi check-config && systemctl --user restart uptime-kinomi

``check-config`` now warns:

.. code-block:: text

   warning: /home/pi/.config/uptime-kinomi/kinomi.toml: private pages on a non-loopback address: without a TLS proxy, passwords travel unencrypted

That is true: in your home network, the password travels without
encryption. That is acceptable at home. Before you make kinomi reachable
from the internet, put a web server with HTTPS in front of it
(:doc:`/operations/reverse-proxy`).

Open ``http://raspberrypi.local:8080/private/all/``. The browser asks for
the user name and password. Private pages live below ``/private/``.

6. Did the backup run?
----------------------

kinomi can also wait to be called. A **push** monitor expects your backup
script (or any cron job) to call a secret URL when it has finished. If the
call does not come in time, the monitor goes DOWN and you get notified. This
finds the failure nobody notices: the backup that silently stopped running
weeks ago.

First, a random secret token for the URL:

.. code-block:: sh

   uptime-kinomi gen-token

.. code-block:: text

   3T1YbhlYipY4nh5R27mMQ7DONHlW2cAJDj2cq2vyGL4

Then the monitor, ``monitors/backup.toml``, with your token:

.. literalinclude:: /examples/tutorial/monitors/backup.toml
   :language: toml
   :caption: monitors/backup.toml

``interval = "1d"`` means: one call per day is expected. ``grace = "2h"`` in ``[push]``
allows the backup to finish up to two hours later than the day before.
``retries = 0`` makes the monitor DOWN at the first missing call; with the
default of one retry you would only hear about it a day later.

The token is a secret, so make the file readable only for you:

.. code-block:: sh

   chmod 600 monitors/backup.toml
   uptime-kinomi check-config && systemctl --user restart uptime-kinomi

Try the URL by hand, on the Pi:

.. code-block:: sh

   curl -fsS http://localhost:8080/push/3T1YbhlYipY4nh5R27mMQ7DONHlW2cAJDj2cq2vyGL4

``curl`` prints nothing (kinomi answers ``204 No Content``), and the log
shows:

.. code-block:: text

   2026-11-01T10:45:24Z monitor=backup result=UP msg=push

Finally, add the same line **at the end of your backup script**, after the
backup succeeded. If the backup runs on another computer, use the Pi's
address instead of ``localhost``.

.. code-block:: sh

   # ... the backup itself ...
   curl -fsS -m 10 --retry 3 http://raspberrypi.local:8080/push/3T1YbhlYipY4nh5R27mMQ7DONHlW2cAJDj2cq2vyGL4

The push monitor guide shows more patterns, such as reporting failures
right away (:doc:`/monitors/push`).

The complete configuration
--------------------------

This is what ``kinomi.toml`` looks like now (your host names, addresses
and time zone differ):

.. literalinclude:: /examples/tutorial/kinomi.toml
   :language: toml
   :caption: kinomi.toml

And the directory:

.. code-block:: text

   ~/.config/uptime-kinomi/
   ├── kinomi.toml
   ├── htpasswd
   ├── monitors/
   │   ├── backup.toml
   │   ├── internet-dns.toml
   │   ├── router.toml
   │   └── website.toml
   ├── notifications/
   │   └── phone.toml
   └── pages/
       ├── all.toml
       └── home.toml

Where to go from here
---------------------

* :doc:`concepts` explains states, retries and the configuration rules in
  more depth.
* :doc:`/monitors/index` has examples for every kind of monitor.
* :doc:`/notifications/index` connects Gotify, Telegram, Discord, Slack and
  others.
* :doc:`/guides/status-pages` builds nicer pages, with groups, badges and
  feeds.
* :doc:`/operations/reverse-proxy` makes the page reachable from the
  internet, with HTTPS.
