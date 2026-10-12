<!doctype html>
<html>
<body style="font-family: Helvetica, Arial, sans-serif; color: #1a1a1a; line-height: 1.5;">
  <p>Hi {{ name | escape }},</p>
  <p>The Collaboration Tool password for <strong>{{ who | escape }}</strong> in {{ org | escape }} was just reset, at {{ at | escape }}. {{ who | escape }} can see shared login passwords, so you are told too.</p>
  <p>If {{ who | escape }} did this, there is nothing else to do. If not, someone may have got into their email. Ask them, and check Login activity.</p>
  <p><a href="{{ activity_url | escape }}" style="display: inline-block; padding: 10px 18px; border-radius: 999px; background: #ffb454; color: #000; text-decoration: none;">Open Login activity</a></p>
</body>
</html>
