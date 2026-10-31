Installation
============

kinomi is a single program file. Installing it means: put that file on
your computer, write a configuration, and start it as a service. There are
three ways to do this. Pick the one that fits you:

.. list-table::
   :header-rows: 1
   :widths: 25 45 30

   * - Way
     - Good for
     - Needs
   * - :doc:`install-script`
     - Most people. One command sets up everything, including automatic
       updates.
     - A Linux computer with systemd (Raspberry Pi OS, Debian, Ubuntu, ...)
   * - :doc:`manual`
     - You want to see and do every step yourself, have no systemd, or want
       a system-wide service with its own user.
     - A shell
   * - :doc:`ansible`
     - Several Pis, or a configuration you keep in Git and deploy from your
       laptop.
     - Ansible on your laptop, SSH access to the Pi

**None of them needs root.** kinomi runs as your normal user (for example
``pi``) and keeps everything in your home directory. The only exception is
the optional system-wide installation described in :doc:`manual`.

.. toctree::
   :maxdepth: 1

   install-script
   manual
   ansible

What you need
-------------

* A computer with **Linux**: any Raspberry Pi (including the very first Pi 1
  and the Pi Zero), another single-board computer, a PC, a virtual machine
  or a server. Windows and macOS are not supported.
* A network connection, so kinomi can reach what it monitors.
* About 4 MB of disk space for the program and a few MB for its logs.

.. _which-binary:

Which binary do I need?
-----------------------

kinomi is published as five program files, one per processor type. The
install script and the Ansible roles pick the right one for you. For a manual
installation, run ``uname -m`` on the computer and look it up here:

.. list-table::
   :header-rows: 1
   :widths: 45 15 40

   * - Your board / system
     - ``uname -m``
     - File
   * - Raspberry Pi 1 (A, B, A+, B+), Pi Zero, Pi Zero W
     - ``armv6l``
     - ``uptime-kinomi-armv6``
   * - Raspberry Pi 2, 3, 4, 5, Zero 2 W with a **32-bit** system; Banana
       Pi M1/M2/M3, Orange Pi PC/One/Zero, BeagleBone
     - ``armv7l``
     - ``uptime-kinomi-armv7`` (``armv6`` works too)
   * - Raspberry Pi 3, 4, 5, Zero 2 W with a **64-bit** system; Banana Pi
       M5/M7, Orange Pi 5, Rock Pi, Odroid
     - ``aarch64``
     - ``uptime-kinomi-aarch64``
   * - PC, NUC, virtual machine, server
     - ``x86_64``
     - ``uptime-kinomi-x86_64``
   * - VisionFive 2, Banana Pi BPI-F3 (best effort)
     - ``riscv64``
     - ``uptime-kinomi-riscv64``

The files are static: they do not depend on the libraries of your system,
so one file works on every distribution.

Where things end up
-------------------

All three ways put files in the same places (for the user ``pi``):

.. list-table::
   :widths: 40 60

   * - ``~/.local/bin/uptime-kinomi``
     - the program
   * - ``~/.config/uptime-kinomi/``
     - your configuration: one file per monitor, notifier and page
   * - ``~/.local/state/uptime-kinomi/log/``
     - the check log, one file per day
   * - ``~/.local/state/uptime-kinomi/state/``
     - the current state of every monitor (rebuilt from the log if lost)
   * - ``/run/user/1000/uptime-kinomi/www/``
     - the generated status pages, in memory (they change every minute, and
       this spares the SD card)
   * - ``~/.config/systemd/user/``
     - the service that starts kinomi at boot

``uptime-kinomi check-config`` prints these directories for your setup.
