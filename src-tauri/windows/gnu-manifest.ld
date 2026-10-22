/* Some MinGW distributions inject a manifest without Common Controls v6.
   Keep the application's manifest as the only activation context. */
SECTIONS { /DISCARD/ : { *default-manifest.o(.rsrc) } }
INSERT BEFORE .rsrc;
