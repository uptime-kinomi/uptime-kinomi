How kinomi works
================

This page explains the ideas behind kinomi: the configuration files, the
states a monitor can be in, and where kinomi keeps its data. You don't need
it to get started, but it answers most "why does it do that?" questions.

The configuration directory
---------------------------

Everything kinomi does is described by small text files in one directory,
usually ``~/.config/uptime-kinomi`` (``/etc/uptime-kinomi`` for a
system-wide installation):

.. code-block:: text

   ~/.config/uptime-kinomi/
   ├── kinomi.toml          settings for kinomi itself
   ├── htpasswd             passwords for private pages (optional)
   ├── custom.css           your own styles for the pages (optional)
   ├── monitors/            one file per monitor
   ├── notifications/       one file per notification channel
   ├── pages/               one file per status page
   ├── incidents/           one file per announcement (optional)
   ├── maintenance/         one file per maintenance window (optional)
   └── secrets/             tokens and passwords, if you like (any name works)

The files are in `TOML <https://toml.io>`_ format: ``key = value`` lines,
grouped into ``[sections]``. Text goes in quotes, numbers and ``true`` /
``false`` don't, lists are written in square brackets.

The rules are few:

* **The file name is the ID.** ``monitors/website.toml`` defines the
  monitor ``website``. Other files refer to it by that ID, e.g. in a page.
  IDs use lowercase letters, digits, ``-`` and ``_``.
* **Every file starts with** ``config_version = 1``.
* **Unknown keys are errors.** A typo never goes unnoticed.
* **Durations have a unit:** ``"500ms"``, ``"30s"``, ``"5m"``, ``"2h"``,
  ``"1d"``.
* **Relative paths are relative to the configuration directory**, so you
  can copy the directory to another Pi or another user as it is.
* Files that don't end in ``.toml`` and hidden files are ignored, so
  editor backups don't matter.

Edit, check, restart
--------------------

kinomi reads the configuration once, at startup. After a change:

.. code-block:: sh

   uptime-kinomi check-config && systemctl --user restart uptime-kinomi

``check-config`` validates everything: the syntax, the values, references
between files (a page listing a monitor that does not exist), duplicate push
tokens, and file permissions of secrets. It exits with an error if something
is wrong, so the restart does not happen. Startup takes less than a second,
even on a Pi 1, and kinomi continues exactly where it stopped.

Secrets
-------

Tokens and passwords can be written into a file directly
(``token = "..."``) or kept in a separate file (``token_file =
"secrets/ntfy.token"``). Separate files are easier to keep out of Git and
to protect. kinomi warns when a file that contains a secret is readable by
other users of the computer:

.. code-block:: text

   warning: ./notifications/gotify.toml: gotify.token_file ./secrets/gotify.token is readable by all users (chmod o-r)

Fix it with ``chmod 600 secrets/*`` (or ``chmod o-r`` on the file named).

.. _states:

States of a monitor
-------------------

.. list-table::
   :header-rows: 1
   :widths: 20 55 25

   * - State
     - Meaning
     - Counts as
   * - ✓ ``UP``
     - The last check succeeded.
     - up
   * - ⚠ ``WARN``
     - It succeeded, but something needs attention: the certificate expires
       soon, or the response was slow.
     - up
   * - … ``PENDING``
     - The last check failed, but kinomi tries again before it believes it
       (see *retries* below).
     - up
   * - ✗ ``DOWN``
     - It failed several times in a row. You get a notification.
     - down
   * - ⚙ ``MAINTENANCE``
     - Inside a maintenance window: checks run, but don't count and don't
       notify.
     - not counted
   * - ‖ ``PAUSED``
     - The monitor is switched off in its file (``enabled = false``).
     - not counted
   * - ? ``NO DATA``
     - No check result yet, right after it was added.
     - not counted

The uptime percentages on the pages count *up* and *down* results; the
others are left out.

Retries: when does a failure count?
-----------------------------------

A single failed check is often a hiccup: a dropped packet, a server that
was busy for a second. Therefore kinomi only reports DOWN after
``retries + 1`` failed checks **in a row**. With the default ``retries =
1`` and a check every minute:

.. code-block:: text

   10:00  check OK       UP
   10:01  check fails    PENDING   (1 failure, 1 retry left)
   10:02  check fails    DOWN      notification "My blog is DOWN"
   10:03  check fails    DOWN
   10:04  check OK       UP        notification "My blog is UP again", after 2 min down

A check that succeeds in between resets the count. ``retries = 0`` reports
DOWN at the first failure; ``retries = 3`` waits for four failures. For
push monitors, which are only called once a day, ``retries = 0`` is usually
right (:doc:`/monitors/push`).

Notifications
-------------

A monitor sends a notification to each channel listed in its
``notifications`` (or in ``[defaults]``) when:

* it goes **DOWN**;
* it is **UP** again (with how long it was down);
* it gets a **warning** (at most once a day per monitor);
* it is still DOWN after ``resend_interval``, if you set one (a
  **reminder**).

There are no notifications during maintenance windows. After a restart,
kinomi remembers which monitors were DOWN and does not notify again.

How checks are scheduled
------------------------

Every monitor is checked on a fixed grid: every ``interval``. The first
checks are spread over the first 10 seconds after startup, so that not all
checks run at once. Four checks can run at the same time (``workers`` in
``kinomi.toml``). If a slow Pi cannot keep up, kinomi skips checks fairly,
so that every monitor is checked equally often, and logs it as
``result=SKIPPED``. A check never takes longer than its ``timeout``.

Where kinomi keeps its data
---------------------------

**The log** (``~/.local/state/uptime-kinomi/log/``) is the record of
everything: one file per day (UTC), one line per check result and event. It
is plain text you can read, ``grep`` and ``awk``
(:doc:`/operations/logs-and-data`). Old files are deleted after 90 days
(``log_retention``).

**The state files** (``~/.local/state/uptime-kinomi/state/``) hold the
current state and the history of every monitor (the heartbeat bar, uptime
per hour and day). They are written every 10 minutes and at shutdown, and
can be rebuilt from the log at any time.

**The pages** are generated about once a minute, into memory
(``/run/user/<uid>/uptime-kinomi/www``), because they change constantly and
writing them to an SD card would wear it out. They are regenerated within a
minute if they are lost.

kinomi writes to the SD card only every 30 seconds (the log) and every 10
minutes (the state files): about 0.3 MB per 10 minutes for 50 monitors.

Public and private
------------------

Every page is either **public** (anyone who can reach kinomi may see it) or
**private** (behind a user name and password, below ``/private/``). Public
pages show names, states and uptime only: never URLs, host names, error
messages or descriptions. Private pages show all of that, plus kinomi's own
health. Each page's ``status.json``, feed and badges have the same
visibility as the page.

What kinomi never does
----------------------

* It never shows or forwards what it receives from the things it checks.
  Pages, logs and notifications only contain kinomi's own summary, such as
  ``HTTP 503`` or ``keyword missing``. A hacked website cannot inject
  anything into your status page.
* It never runs JavaScript on its pages. They are plain HTML and CSS and
  work in any browser, even in text browsers.
* It never needs root.
