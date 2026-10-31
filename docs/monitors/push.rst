Push monitors
=============

A ``push`` monitor turns the usual direction around: kinomi does not check
anything, it **waits to be called**. A backup script, a cron job or another
computer calls a secret URL when it has done its work. If the call does not
arrive in time, the monitor fails.

This finds the failures nobody notices, because nothing happens: the
backup that stopped running three weeks ago, the cron job that was lost when
the server was reinstalled.

How it works
------------

#. You create a push monitor with a secret token and an ``interval``: how
   often a call is expected.
#. The job calls ``http://<your-kinomi>/push/<token>`` when it has finished.
#. Every call starts a new countdown of ``interval + grace``. If it runs
   out, the check fails (``no push received``), and again after every
   further ``interval`` without a call. ``retries`` applies as for every
   monitor.

Push monitors need kinomi's built-in web server, and the job must be able
to reach it.

The nightly backup
------------------

First, a random token:

.. code-block:: sh

   uptime-kinomi gen-token

.. code-block:: text

   3T1YbhlYipY4nh5R27mMQ7DONHlW2cAJDj2cq2vyGL4

Put it into the monitor file:

.. literalinclude:: /examples/catalog/monitors/backup.toml
   :language: toml
   :caption: monitors/backup.toml

* ``interval = "1d"``: one call per day.
* ``grace = "2h"`` (in ``[push]``): a backup that takes up to two hours longer than the
  day before is still on time. Without ``grace``, one minute is allowed.
* ``retries = 0``: DOWN at the first missed call. With the default
  ``retries = 1``, kinomi would tolerate one missed day and tell you only
  after the second one.
* ``resend_interval = "12h"``: remind me twice a day while it stays DOWN.
* ``token``: the secret part of the URL. The file now contains a secret,
  so make it readable only for you: ``chmod 600 monitors/backup.toml``
  (``check-config`` warns otherwise). If you prefer to keep secrets in
  separate files, use the commented-out ``token_file`` instead (see
  `Details`_).

At the end of the backup script, after the backup succeeded:

.. code-block:: sh
   :caption: backup.sh

   #!/bin/sh
   set -e
   # ... the backup itself, e.g.:
   restic backup /home
   # Only reached when everything above succeeded (set -e):
   curl -fsS -m 10 --retry 3 https://status.example.org/push/3T1YbhlYipY4nh5R27mMQ7DONHlW2cAJDj2cq2vyGL4 >/dev/null

``-m 10`` gives up after 10 seconds and ``--retry 3`` tries again on a
network hiccup, so the backup script never hangs because of kinomi.

Without ``curl``, any HTTP client works (``wget -q -O /dev/null URL``):
kinomi accepts ``GET`` and ``POST``.

An hourly cron job
------------------

.. literalinclude:: /examples/catalog/monitors/cron-hourly.toml
   :language: toml
   :caption: monitors/cron-hourly.toml

Here the default ``retries = 1`` makes sense: one missed hour is PENDING,
two in a row are DOWN. The call can go straight into the crontab line, so
the job itself does not change:

.. code-block:: text
   :caption: crontab -e

   15 * * * *  /usr/local/bin/import.sh && curl -fsS -m 10 --retry 3 https://status.example.org/push/PASTE-THE-TOKEN-HERE >/dev/null

``&&`` calls kinomi only when the job succeeded.

Reporting failures right away
-----------------------------

A script that knows it failed can say so: ``?status=down`` counts as a
failed check immediately, instead of waiting for the deadline. This example
checks the free disk space every hour:

.. literalinclude:: /examples/catalog/monitors/disk-space.toml
   :language: toml
   :caption: monitors/disk-space.toml

.. code-block:: sh
   :caption: check-disk.sh, run hourly from cron

   #!/bin/sh
   url=https://status.example.org/push/PASTE-THE-TOKEN-HERE
   used=$(df --output=pcent / | tail -n 1 | tr -dc 0-9)
   if [ "$used" -lt 90 ]; then
       curl -fsS -m 10 --retry 3 "$url?status=up" >/dev/null
   else
       curl -fsS -m 10 --retry 3 "$url?status=down" >/dev/null
   fi

kinomi shows ``push: status=down`` as the reason. If the script stops
running altogether, the deadline still catches it.

Two Pis watching each other
---------------------------

kinomi can call a push URL itself, regularly, with the ``[heartbeat]``
section of ``kinomi.toml``. Point the heartbeat of one kinomi at a push
monitor on another one, and the other way round, and each one notices when
the other is gone:

.. literalinclude:: /examples/catalog/monitors/other-pi.toml
   :language: toml
   :caption: monitors/other-pi.toml

:doc:`/operations/two-pis` walks through the whole setup.

Pausing a job
-------------

If a job is switched off on purpose, pause its monitor instead of deleting
it. A disabled push monitor answers calls with ``404``, like an unknown
token:

.. literalinclude:: /examples/catalog/monitors/photos-sync.toml
   :language: toml
   :caption: monitors/photos-sync.toml

Details
-------

**The URL.** ``GET`` or ``POST`` to ``/push/<token>``. kinomi answers:

.. list-table::
   :widths: 25 75

   * - ``204 No Content``
     - Accepted.
   * - ``404 Not Found``
     - Unknown token, or the monitor is disabled.
   * - ``429 Too Many Requests``
     - More than one call per second for this token.
   * - ``400 Bad Request``
     - ``status`` is neither ``up`` nor ``down``.
   * - ``405 Method Not Allowed``
     - ``HEAD`` or another method. This keeps link previews in chat apps
       from counting as a push.

**Query parameters.** ``status=up`` (the default) or ``status=down``. The
parameters ``msg`` and ``ping`` of uptime-kuma push URLs are accepted, but
ignored: kinomi never stores what a client sends. A job moved from
uptime-kuma can keep its token (32 letters and digits, valid here too); only
the path changes from ``/api/push/<token>`` to ``/push/<token>``.

**Tokens** are 32 to 128 characters from ``A-Z a-z 0-9 _ -`` and must be
unique. ``uptime-kinomi gen-token`` prints a random one. Write it into the
monitor file as ``token = "..."`` (and ``chmod 600`` the file), or keep it
in a file of its own and refer to it with ``token_file``:

.. code-block:: sh

   mkdir -p secrets
   uptime-kinomi gen-token > secrets/backup.token
   chmod 600 secrets/backup.token

.. code-block:: toml

   [push]
   token_file = "secrets/backup.token"   # relative to the configuration directory

A separate file is handy when the configuration is in Git and the secrets
are not. Anyone who knows the token can report "all good", so treat it like a
password.

**After a restart** the countdown continues where it was, counted from the
last call. A call that arrived while kinomi was not running is lost, so
kinomi waits at least ``grace`` after its start before it counts a missed
call.

**During a maintenance window** a missed call does not count, but a call
with ``status=up`` is always processed: if the job reports success, kinomi
believes it.

**Keys:**

.. list-table::
   :header-rows: 1
   :widths: 25 20 55

   * - Key
     - Default
     - Meaning
   * - ``interval`` (top level)
     - from ``[defaults]``
     - How often a call is expected.
   * - ``grace`` (in ``[push]``)
     - ``"1m"``
     - Extra time before the first missed call counts; at most ``1d``.
   * - ``token`` / ``token_file``
     - (one required)
     - The secret part of the URL.

Push monitors have no ``timeout``: there is nothing to wait for.
