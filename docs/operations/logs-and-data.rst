Logs and data
=============

kinomi keeps no database. Everything it knows is in text files you can read
with the usual tools.

Where is what?
--------------

.. code-block:: sh

   uptime-kinomi check-config | head -n 4

.. code-block:: text

   config dir: /home/pi/.config/uptime-kinomi
   data dir:   /home/pi/.local/state/uptime-kinomi
   log dir:    /home/pi/.local/state/uptime-kinomi/log
   output dir: /run/user/1000/uptime-kinomi/www

.. list-table::
   :widths: 30 70

   * - log directory
     - The check log: the record of everything. Back it up.
   * - ``state/`` in the data directory
     - The current state of each monitor, derived from the log.
   * - output directory
     - The generated pages. Regenerated at every start; nothing to back up.

To change them, set ``data_dir`` and ``log_dir`` in ``[storage]`` and
``output_dir`` in ``[pages]`` of ``kinomi.toml``.

kinomi's own messages (startup, errors) go to the journal:
``journalctl --user -u uptime-kinomi``.

The check log
-------------

One file per day, named after the date in UTC
(``2026-11-01.log``). One line per event, timestamp first, then
``key=value`` pairs (the `logfmt <https://brandur.org/logfmt>`_ format):

.. code-block:: text

   2026-11-01T14:00:00Z event=START version=0.8.0 monitors=42
   2026-11-01T14:00:00Z event=DISABLED monitor=old-nas
   2026-11-01T14:03:11Z monitor=blog result=UP latency_ms=123 cert_expires=2026-12-01 msg="HTTP 200"
   2026-11-01T14:04:11Z monitor=shop result=FAIL latency_ms=10000 msg=timeout
   2026-11-01T14:05:11Z monitor=shop result=DOWN latency_ms=10000 msg=timeout
   2026-11-01T14:05:12Z event=NOTIFY monitor=shop notifier=phone kind=down ok=true
   2026-11-01T14:06:00Z monitor=nas result=SKIPPED count=1 msg="queue full"
   2026-11-01T14:06:02Z monitor=backup-job result=UP msg=push
   2026-11-01T14:07:30Z event=AUTH_FAIL count=3 throttled=0
   2026-11-01T22:00:00Z event=STOP

``result`` is one of:

.. list-table::
   :widths: 20 80

   * - ``UP``
     - The check succeeded.
   * - ``WARN``
     - It succeeded with a warning (certificate, slow response).
   * - ``FAIL``
     - It failed, but retries are left (the monitor is PENDING).
   * - ``DOWN``
     - It failed and the monitor is DOWN.
   * - ``MAINT``
     - A check inside a maintenance window.
   * - ``SKIPPED``
     - Checks that could not run: ``queue full`` (too much to do) or
       ``paused`` (the whole system was suspended).

Lines with ``event=`` are kinomi's own events: ``START`` and ``STOP``,
``NOTIFY`` (a notification and whether it arrived), ``AUTH_FAIL`` (wrong
passwords, summed up), ``HEARTBEAT`` (the outgoing heartbeat),
``CLOCK_JUMP`` (the system clock was changed), ``PAUSED`` (the system was
suspended), ``DISABLED`` (a disabled monitor at startup), and
``LOG_LINES_DROPPED`` (more than 10 000 lines were waiting to be written).

kinomi collects lines in memory and writes them every 30 seconds
(``flush_interval``), to spare the SD card. A line can therefore appear up
to 30 seconds after the event. Files older than 90 days are deleted
(``log_retention``).

Reading the log
---------------

Today's log, live:

.. code-block:: sh

   cd ~/.local/state/uptime-kinomi/log
   tail -f $(date -u +%F).log

Everything that went wrong this month:

.. code-block:: sh

   grep -h 'result=DOWN' 2026-11-*.log

When did the shop go down and come back?

.. code-block:: sh

   grep -h 'monitor=shop ' *.log | grep -E 'result=(DOWN|UP)' | awk '$3 != prev { print; prev = $3 }'

Notifications that did not arrive:

.. code-block:: sh

   grep -h 'event=NOTIFY.*ok=false' *.log

Average response time of the blog per day:

.. code-block:: sh

   for f in *.log; do
       grep -h 'monitor=blog result=UP' "$f" |
           sed -n 's/.*latency_ms=\([0-9]*\).*/\1/p' |
           awk -v d="${f%.log}" '{ s += $1; n++ } END { if (n) printf "%s %d ms\n", d, s / n }'
   done

Times are in UTC, so a day's file starts at 01:00 in Berlin in winter.

The state files
---------------

``state/<monitor>.toml`` in the data directory holds what the pages need:
the current state and since when, the last result, the heartbeat bar,
hourly totals for the last two days and daily totals for 400 days
(``history_retention``). They are written every 10 minutes
(``state_interval``) and when kinomi stops.

At startup, kinomi loads the state files and reads the log lines that came
after them. If a state file is missing or broken, kinomi rebuilds it from
the log. To rebuild all of them, stop kinomi first:

.. code-block:: sh

   systemctl --user stop uptime-kinomi
   uptime-kinomi rebuild-state
   systemctl --user start uptime-kinomi

A rebuild only knows what the log still has (90 days by default). Daily
totals older than that are lost, so keep the state files in your backups.

The SD card
-----------

SD cards wear out with writes. kinomi is careful:

* the log is written every 30 seconds, not with every line;
* the state files every 10 minutes;
* the pages, which change with every check, go to memory
  (``/run/user/<uid>``), not to the card.

For 50 monitors checked every minute, kinomi writes about 0.3 MB per 10
minutes to the card. Without ``/run/user/<uid>`` (no systemd user session,
e.g. started from ``crontab``), the pages are written into the data
directory; then set ``output_dir`` to a directory in memory, or
``render_interval = "5m"``.

Clock
-----

Raspberry Pis have no battery-backed clock. Until the network time is set
after boot, the time can be wrong. kinomi:

* waits at startup until the clock shows a plausible date (after
  2026-01-01), so the log never contains dates in 1970;
* schedules checks with a clock that is not affected by changes of the
  time;
* logs ``event=CLOCK_JUMP delta_s=...`` when the clock jumps by more than a
  minute.
