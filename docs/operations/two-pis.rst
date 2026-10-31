Who watches the watcher?
========================

If the Pi that runs kinomi fails (power, SD card, network), it cannot tell
you. Something outside has to notice that kinomi is gone. kinomi can send a
regular "I am alive" signal for that: the **heartbeat**.

Two Pis watching each other
---------------------------

Two kinomi instances, ideally in different places (home and a friend's
house, or home and a small cloud server), each send a heartbeat to the
other one, and each has a push monitor for the other's heartbeat. When one
fails, the other notices and notifies you.

The example uses ``pi-a`` (``https://status-a.example.org``) and ``pi-b``
(``https://status-b.example.org``). Both must be reachable for each other,
usually through a reverse proxy (:doc:`reverse-proxy`).

**Step 1: on pi-b, create a push monitor for pi-a.**

.. code-block:: sh

   cd ~/.config/uptime-kinomi
   mkdir -p secrets
   uptime-kinomi gen-token > secrets/pi-a.token
   chmod 600 secrets/pi-a.token
   cat secrets/pi-a.token

.. code-block:: toml
   :caption: pi-b: monitors/pi-a.toml

   config_version = 1
   type = "push"
   name = "Pi A"
   interval = "60s"           # pi-a's heartbeat interval
   grace = "30s"

   [push]
   token_file = "secrets/pi-a.token"

**Step 2: on pi-a, send the heartbeat to it.** Use the token printed in
step 1:

.. code-block:: toml
   :caption: pi-a: kinomi.toml

   [heartbeat]
   url = "https://status-b.example.org/push/PASTE-THE-TOKEN-HERE"
   interval = "60s"

The URL contains the token, so protect the file: ``chmod 600 kinomi.toml``.

**Step 3: the same in the other direction:** a push monitor ``pi-b`` on
pi-a, and a heartbeat to it in pi-b's ``kinomi.toml``.

Restart both. In each log you see the heartbeat's first result:

.. code-block:: text

   2026-11-01T10:00:03Z event=HEARTBEAT ok=true

and later only changes, e.g. ``ok=false msg="timeout"`` when the other Pi
is unreachable, so a long outage does not fill the log.

With a hosted service
---------------------

Without a second Pi, use a service that alerts when a regular call stops,
such as `healthchecks.io <https://healthchecks.io>`_ (free for a few
checks). Create a check with a period of 1 minute and a grace time of a few
minutes, and put its ping URL into ``[heartbeat]``:

.. code-block:: toml
   :caption: kinomi.toml

   [heartbeat]
   url = "https://hc-ping.com/your-uuid"
   interval = "60s"

The heartbeat is a ``GET`` request every ``interval`` (plus the time the
request takes), with a timeout of 15 seconds.

Checking the health from outside
--------------------------------

``/healthz`` answers ``200 ok`` when kinomi's scheduler and event loop run,
and ``503`` when they are stuck. An HTTP monitor on the other Pi can check
it too:

.. code-block:: toml

   config_version = 1
   type = "http"
   name = "Pi A health"

   [http]
   url = "https://status-a.example.org/healthz"

The heartbeat tells you that the process lives; ``/healthz`` that it also
works and is reachable.
