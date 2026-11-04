Troubleshooting
===============

Three commands answer most questions:

.. code-block:: sh

   uptime-kinomi check-config                         # is the configuration OK?
   systemctl --user status uptime-kinomi              # is it running?
   journalctl --user -u uptime-kinomi -n 50           # what did it say?

and the check log shows what kinomi saw:

.. code-block:: sh

   tail -n 30 ~/.local/state/uptime-kinomi/log/$(date -u +%F).log

.. _empty-journal:

The journal is empty
--------------------

``journalctl --user -u uptime-kinomi`` (or ``-u uptime-kinomi-update``)
says "No journal files were found" or shows nothing. Raspberry Pi OS keeps
the journal only in RAM, to spare the SD card: it is small, its oldest
messages are dropped after a few hours, all of it is gone after a reboot,
and ``journalctl --user`` finds nothing at all, because separate journals
for each user only exist on disk. Messages that are still in RAM show up
with ``--user-unit`` instead:

.. code-block:: sh

   journalctl --user-unit uptime-kinomi-update -n 20

To keep the journal on disk, which most other Linux distributions do
anyway (an administrator; the file name must sort after Raspberry Pi OS's
``40-rpi-volatile-storage.conf``):

.. code-block:: sh

   sudo mkdir -p /etc/systemd/journald.conf.d
   printf '[Journal]\nStorage=persistent\nSystemMaxUse=50M\n' |
     sudo tee /etc/systemd/journald.conf.d/50-persistent.conf
   sudo systemctl restart systemd-journald

From then on, ``journalctl --user -u ...`` works, including for messages
from before a reboot. The journal writes a few KB per hour to the SD card;
``SystemMaxUse`` limits its size. The check log
(``~/.local/state/uptime-kinomi/log/``) is not affected: kinomi always
writes it to disk.

.. _page-does-not-open:

The status page does not open
-----------------------------

**From another computer, the browser says the connection was refused or
timed out.** kinomi only listens on the Pi itself (``127.0.0.1``), which is
the default. To open the page from your network, set in ``kinomi.toml``:

.. code-block:: toml

   [server]
   listen = "0.0.0.0:8080"

and restart. Test on the Pi itself first: ``curl -sI http://localhost:8080/``.

**The page says "Misdirected Request" (HTTP 421).** kinomi only answers for
the names in ``hostnames``, to protect against DNS rebinding attacks. The
page itself says what to do: add the name or address you typed into the
browser:

.. code-block:: toml

   [server]
   hostnames = ["localhost", "raspberrypi.local", "192.168.1.50"]

This happens typically **after the Pi got a new IP address** from your
router. A fixed address for the Pi (a "DHCP reservation" in the router)
prevents it, and so does using the name (``raspberrypi.local``) instead of
the address.

**"Not Found" (HTTP 404) at "/".** There is no public page, and no
``default_page``. Open a private page directly (``/private/<id>/``) or set
``default_page``.

**The browser keeps asking for the password.** The user is not in the
htpasswd file, the password is wrong, or the file is not the one in
``htpasswd_file``. After 5 wrong attempts in a row, kinomi only checks one
password per second and answers others with ``429``; wait a moment.

kinomi does not start
---------------------

``systemctl --user status uptime-kinomi`` shows ``failed``:

.. code-block:: sh

   journalctl --user -u uptime-kinomi -n 20

* **A configuration error:** ``uptime-kinomi check-config`` shows it, with
  file and line.
* **"cannot listen on 127.0.0.1:8080: Address already in use (os error
  98)":** another program uses port 8080, or a
  second kinomi runs (perhaps one you started by hand with ``uptime-kinomi
  run``). ``ss -ltnp | grep 8080`` shows which. Change ``listen`` to
  another port, or stop the other program.
* **It waits for the clock:** a Pi without network time shows a date
  before 2026 until NTP has synchronised. kinomi waits until then and says
  so in the journal (``the system clock ... is not set yet; waiting for
  NTP``). Check ``timedatectl``.

kinomi stops when I log out
---------------------------

*Lingering* is not enabled for your user, so your user services only run
while you are logged in:

.. code-block:: sh

   loginctl show-user $USER -p Linger      # Linger=no
   loginctl enable-linger                  # or, by an administrator: sudo loginctl enable-linger pi

A monitor is DOWN but the service works
---------------------------------------

Look at the message on the private page or in the log; the monitor guides
explain each one (:doc:`/monitors/http`, :doc:`/monitors/tcp`,
:doc:`/monitors/dns`).

**It works from my laptop, not from the Pi.** kinomi checks from the Pi.
The Pi may resolve the name differently (``getent hosts example.org``), be
blocked by a firewall, or have no IPv6 route. Try the same from the Pi:
``curl -sI https://example.org/``.

**"TLS certificate from unknown issuer"** for a device in your network: it
uses a self-signed certificate. See the router example in
:doc:`/monitors/http`.

**"keyword missing" although the word is on the page.** The keyword is
compared with the HTML source, case-sensitively, within the first 64 KiB.
Look at the source (``curl -s URL | grep -c 'your keyword'``): the text
might be written with HTML entities, split by tags, or loaded later by
JavaScript (which kinomi does not run).

**"HTTP 301" or "HTTP 302".** The URL redirects. Put the final URL into the
monitor, or allow redirects with ``max_redirects``.

**The monitor is PENDING for a long time.** PENDING means "failed, but
retries are left". With a long ``interval``, waiting for the next check
takes that long. Lower ``retries`` or the interval.

Push monitors
-------------

**The job gets 404.** The token in the URL does not match, the monitor is
disabled (``enabled = false``), or kinomi was not restarted after adding
the monitor. Note that the token is the content of the token file, not its
name.

**The job gets 421.** The host name in the URL is not in ``hostnames``.

**The job gets 405.** It sends ``HEAD`` (e.g. ``curl -I``). Use ``GET``
(plain ``curl``) or ``POST``.

**The job gets 429.** More than one call per second for the same token.

**The monitor is DOWN although the job ran.** Check that the call really
arrives: the log shows ``monitor=<id> result=UP msg=push`` for every
accepted call. If the job runs on another computer, check that it can
reach kinomi (``curl -v``).

Notifications do not arrive
---------------------------

.. code-block:: sh

   grep -h 'event=NOTIFY' ~/.local/state/uptime-kinomi/log/*.log | tail

* **No NOTIFY lines at all:** the monitor has no ``notifications`` (and
  ``[defaults]`` has none), the notifier's ``events`` exclude the event, or
  a maintenance window was active.
* **ok=false:** see the ``msg`` and :doc:`/notifications/index`.
* **ok=true, but nothing on the phone:** the message reached the service;
  check the app's subscription (ntfy: the same topic and server) and the
  phone's notification settings.

The Pi is slow, or checks are skipped
-------------------------------------

``result=SKIPPED ... msg="queue full"`` in the log, or skipped checks on a
private page, mean that kinomi has more checks to do than it can run.
Usually some checks take very long (timeouts). Options:

* lower ``timeout`` for monitors in your network;
* raise ``workers`` in ``[scheduler]`` (checks mostly wait for the
  network, so 8 is fine even on a Pi 1);
* raise the ``interval`` of monitors that do not need to be checked every
  minute.

``msg=paused`` instead means that the whole Pi was suspended or frozen
for a while, e.g. a virtual machine that was paused.

Still stuck?
------------

Open an issue at https://github.com/uptime-kinomi/uptime-kinomi/issues with
the output of ``uptime-kinomi version`` and ``uptime-kinomi
check-config``, and the relevant log lines. Remove tokens and passwords
first.
