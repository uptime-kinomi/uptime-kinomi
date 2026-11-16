Command monitors
================

A ``command`` monitor runs a program on the Pi and uses its exit code. It
is the most general check: anything a script can find out, kinomi can
watch. The exit codes follow the convention of Nagios and Icinga plugins,
so the hundreds of existing plugins work as they are. Available since
kinomi 1.2.

.. list-table::
   :header-rows: 1
   :widths: 20 20 60

   * - Exit code
     - Result
     - Message
   * - 0
     - ✓ UP
     - ``exit 0``
   * - 1
     - ⚠ WARN
     - ``exit 1 (warning)``
   * - 2
     - ✗ failure
     - ``exit 2 (critical)``
   * - 3
     - ✗ failure
     - ``exit 3 (unknown)``
   * - other
     - ✗ failure
     - ``exit 4``, ``killed by signal 9``
   * - (too slow)
     - ✗ failure
     - ``timeout``: the program and everything it started are stopped
       (except a process that detached itself with ``setsid``)

As always, a failure becomes DOWN after ``retries``, and WARN counts as up
but sends a ``warn`` notification (at most once a day).

Allowing programs
-----------------

Writing a monitor file normally cannot make kinomi run anything. A
command monitor can, so ``kinomi.toml`` decides which programs are
allowed, and without that list every command monitor is an error:

.. code-block:: toml
   :caption: kinomi.toml

   [commands]
   allow = [
     "/usr/lib/nagios/plugins/check_disk",
     "/usr/lib/nagios/plugins/check_ntp_time",
     "checks/backup-age.sh",
   ]

Relative paths are relative to the configuration directory. An entry
ending in ``/`` allows every program directly in that directory, but list
single programs where you can:

.. warning::

   **Allowing a program allows it with any arguments.** Some programs run
   other programs that are given as arguments: shells and interpreters
   (``sh``, ``python3``), ``env``, ``timeout``, ``find``, ``ssh``, and among
   the monitoring plugins ``negate``, ``urlize`` and ``check_by_ssh``. If
   one of them is allowed, whoever can write a monitor file can run
   anything as kinomi's user. So never allow those, and do not allow the
   whole plugin directory (``/usr/lib/nagios/plugins/``), which contains
   ``negate`` and ``urlize``.

``check-config`` refuses:

* the system program directories ``/bin/``, ``/sbin/``, ``/usr/bin/``,
  ``/usr/sbin/``, ``/usr/local/bin/`` and ``/usr/local/sbin/`` as a whole
  (they contain shells): list single programs from there instead;
* a program that is not allowed, also through a symlink: a link in an
  allowed directory that points to a program outside it is refused;
* paths with ``..``;
* a program, or any directory above it (also of a symlink on the way),
  that another user could change: it must belong to root or to you, and
  must not be writable by group or others (fix with ``chmod g-w,o-w``);
* command monitors when kinomi runs as root.

Disk space, NTP, mail: the monitoring plugins
---------------------------------------------

The ``monitoring-plugins`` package contains ready-made checks. An
administrator installs it once:

.. code-block:: sh

   sudo apt install monitoring-plugins-basic

Then, with ``"/usr/lib/nagios/plugins/check_disk"`` in ``allow``:

.. code-block:: toml
   :caption: monitors/disk.toml

   config_version = 1
   type = "command"
   name = "Free space on the SD card"
   interval = "5m"
   timeout = "30s"

   [command]
   program = "/usr/lib/nagios/plugins/check_disk"
   args = ["-w", "20%", "-c", "10%", "-p", "/"]

WARN below 20 % free space, failure below 10 %. Some more (each one
listed in ``allow``):

.. list-table::
   :header-rows: 1
   :widths: 40 60

   * - What
     - ``program`` and ``args``
   * - The Pi's clock is right (within 0.5 s / 2 s)
     - ``check_ntp_time``, ``["-H", "pool.ntp.org", "-w", "0.5", "-c", "2"]``
   * - A mail server with STARTTLS and its certificate
     - ``check_smtp``, ``["-H", "mail.example.org", "-p", "587", "-S", "-D", "21,7"]``
   * - The load of the Pi
     - ``check_load``, ``["-w", "2,1.5,1", "-c", "4,3,2"]``

``/usr/lib/nagios/plugins/check_disk --help`` (and so on) explains every
plugin's options.

Your own script
---------------

Any executable works: a shell script, Python, a compiled program. Put it
next to the configuration, make it executable, and allow it:

.. code-block:: sh
   :caption: checks/backup-age.sh

   #!/bin/sh
   # Fails when the newest backup is older than a day.
   newest=$(find /srv/backup -name '*.tar.gz' -mtime -1 | head -n 1)
   if [ -n "$newest" ]; then exit 0; else exit 2; fi

.. code-block:: toml
   :caption: monitors/backup-age.toml

   config_version = 1
   type = "command"
   name = "Backup not older than a day"
   interval = "1h"
   timeout = "1m"

   [command]
   program = "checks/backup-age.sh"
   keep_output = true

kinomi runs the program directly, without a shell, so ``args`` are passed
exactly as written: no ``*``, ``$VAR`` or ``|``. Write a script for that.
The program starts in ``/`` with an almost empty environment:
``PATH=/usr/local/bin:/usr/bin:/bin``, ``HOME`` and ``LC_ALL=C``. It
inherits no open files of kinomi, and it is called by the name written in
``program`` (``argv[0]``), also when that is a symlink.

Passwords
---------

Everyone on the Pi can see a running program's arguments (``ps``), so
passwords do not belong into ``args``. Use a program that reads them from
a file (for example a small script that reads the file and calls the
plugin), and protect that file with ``chmod 600``.

Why did it fail? The output
---------------------------

By default kinomi throws away what the program prints: nothing of it is
stored or shown, so a program cannot put text on your pages. To see why a
check fails, run the command yourself:

.. code-block:: sh

   /usr/lib/nagios/plugins/check_disk -w 20% -c 10% -p /; echo "exit $?"

Or keep the output of the last run with ``keep_output = true``. kinomi
then saves what the program printed (stdout and stderr, at most
``output_max_bytes`` from ``[commands]``, 64 KiB by default) to
``private/_output/<monitor>.txt`` in the output directory (``output_dir``),
followed by a line of its own with the result and the time. The private
pages link to it ("output of the last run") as a **download**: the content
is never shown on a page, in notifications or in the log, and the browser
saves the file instead of displaying it. It needs a login like every
private page, so the built-in server only offers it with
``htpasswd_file`` (``check-config`` warns otherwise). Behind a proxy in
mode B, the proxy needs the ``/private/_output/`` block of
:doc:`/operations/reverse-proxy`.

Keys
----

.. list-table::
   :header-rows: 1
   :widths: 25 20 55

   * - Key
     - Default
     - Meaning
   * - ``program``
     - (required)
     - The program, absolute or relative to the configuration directory.
       Must be allowed in ``[commands]``.
   * - ``args``
     - ``[]``
     - Arguments, at most 32, each at most 1024 bytes.
   * - ``keep_output``
     - ``false``
     - ``true``: keep the output of the last run for download.

``timeout`` (from the common keys) is the most the program may take; give
it room, e.g. ``"30s"``.

Messages you may see
--------------------

.. list-table::
   :widths: 40 60

   * - ``exit 0``, ``exit 1 (warning)``, ``exit 2 (critical)``, ``exit 3 (unknown)``, ``exit 4``
     - The program's exit code.
   * - ``killed by signal 9``
     - The program was stopped by a signal (crashed, or killed by the
       system, e.g. out of memory).
   * - ``timeout``
     - It took longer than ``timeout``; kinomi stopped it and everything
       it started.
   * - ``cannot start program``
     - The program was removed or is no longer executable since kinomi
       started.
