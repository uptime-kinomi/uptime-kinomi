Incidents and maintenance
=========================

Monitors measure. Sometimes people need to say something too: "our
provider has an outage, they are working on it", or "the server will be
down on Saturday for an upgrade". kinomi has two kinds of files for that.

* **Incidents** are announcements written by you. They appear on the pages
  and in the feeds, but change nothing about the monitors.
* **Maintenance windows** are planned times when monitors may fail. During
  a window, failures do not count and nobody gets notified of them.

Incidents
---------

Each incident is one file in ``incidents/``. A date in the file name keeps
them in order:

.. literalinclude:: /examples/catalog/incidents/2026-11-02-provider.toml
   :language: toml
   :caption: incidents/2026-11-02-provider.toml

* ``severity``: ``info``, ``minor`` or ``major``; it sets the colour.
* ``start``, ``end``: date and time with a time zone offset (``+01:00``) or
  ``Z`` for UTC. Without ``end``, the incident is *ongoing*.
* ``pages``: where it appears; without it, on every page (with
  ``show_incidents``, the default).
* ``text``: plain text, paragraphs separated by an empty line. No HTML or
  Markdown, at most 4 KiB.

When the problem is solved, add an ``end`` and restart:

.. code-block:: toml

   end = "2026-11-02T11:25:00+01:00"

A resolved incident stays on the page for 7 days, and in the feed for 30
days. Delete the file when you no longer want it at all.

An incident with a ``start`` in the future is shown as *scheduled*, useful
for announcing planned work to the people who read the page:

.. literalinclude:: /examples/catalog/incidents/2026-11-20-move.toml
   :language: toml
   :caption: incidents/2026-11-20-move.toml

.. tip::

   Write the incident first, then restart. Startup takes less than a
   second, and the page shows the incident at the next render (within a
   minute).

Maintenance windows
-------------------

A maintenance window tells kinomi: "these monitors may fail at this time,
and that's fine." During the window the monitors show ⚙ MAINTENANCE:

* checks still run and are written to the log (as ``result=MAINT``), so you
  can see what happened;
* the results count neither as up nor as down in the uptime;
* no notifications are sent (except ``up`` from a push with
  ``status=up``, see below);
* after the window, the monitor continues from its state before the
  window. If it was DOWN before and is UP after, you get the ``up``
  notification.

Pages list the windows of their monitors that are active or start within
the next 7 days.

Each window is one file in ``maintenance/``.

Every week
~~~~~~~~~~

.. literalinclude:: /examples/catalog/maintenance/weekly-backup.toml
   :language: toml
   :caption: maintenance/weekly-backup.toml

``start`` is a date and time in the time zone of ``kinomi.toml``, without
an offset. With ``schedule = "weekly"``, the window repeats every week on the
same weekday and at the same time, from that date on.

Every day
~~~~~~~~~

.. literalinclude:: /examples/catalog/maintenance/nightly-reboot.toml
   :language: toml
   :caption: maintenance/nightly-reboot.toml

``tags`` selects all monitors with that tag, in addition to the ones in
``monitors``. A daily window may be at most one day long.

Once
~~~~

.. literalinclude:: /examples/catalog/maintenance/shop-update.toml
   :language: toml
   :caption: maintenance/shop-update.toml

A window may cross midnight. Around daylight saving time changes, a start
time that does not exist that night moves forward, and one that exists twice
uses the first.

Push monitors in a window
~~~~~~~~~~~~~~~~~~~~~~~~~

A missed push and a ``status=down`` push are logged as ``MAINT`` and do not
count. A push with ``status=up`` (or no status) is the job saying "done", so
it always counts, even in a window, and can bring the monitor back UP (with
the ``up`` notification).

Keys
----

**Incidents:**

.. list-table::
   :header-rows: 1
   :widths: 20 20 60

   * - Key
     - Default
     - Meaning
   * - ``title``
     - (required)
     - 1-200 characters.
   * - ``severity``
     - (required)
     - ``info``, ``minor`` or ``major``.
   * - ``start``
     - (required)
     - RFC 3339 time with offset, e.g. ``2026-11-02T07:40:00+01:00``.
   * - ``end``
     - none (ongoing)
     - Same format.
   * - ``pages``
     - all pages
     - Page IDs.
   * - ``text``
     - none
     - Plain text, at most 4 KiB.

**Maintenance windows:**

.. list-table::
   :header-rows: 1
   :widths: 20 20 60

   * - Key
     - Default
     - Meaning
   * - ``title``
     - (required)
     - Shown on the pages, 1-200 characters.
   * - ``monitors``
     - none
     - Monitor IDs, or ``["*"]`` for all. ``monitors`` or ``tags`` is
       required.
   * - ``tags``
     - none
     - All monitors with one of these tags.
   * - ``schedule``
     - (required)
     - ``once``, ``daily`` or ``weekly``.
   * - ``start``
     - (required)
     - Local time without offset, e.g. ``2026-11-01T03:00:00``.
   * - ``duration``
     - (required)
     - ``1m`` to ``7d`` (``daily``: at most ``1d``).
