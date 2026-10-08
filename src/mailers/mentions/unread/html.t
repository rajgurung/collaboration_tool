<!doctype html>
<html>
<body style="font-family: Helvetica, Arial, sans-serif; color: #1a1a1a; line-height: 1.5;">
  <p>Hi {{ name }},</p>
  <p>{% if count == 1 %}You were mentioned in {{ org }} and haven't seen it yet:{% else %}You have {{ count }} mentions in {{ org }} you haven't seen yet:{% endif %}</p>
  {% for item in items %}
  <div style="margin: 0 0 14px; padding: 12px 14px; border: 1px solid #e2dfd7; border-radius: 12px;">
    <p style="margin: 0;"><strong>{{ item.who }}</strong> {{ item.headline }}</p>
    {% if item.excerpt %}<p style="margin: 6px 0 0; color: #625e57;">“{{ item.excerpt }}”</p>{% endif %}
    <p style="margin: 8px 0 0;"><a href="{{ item.url }}" style="color: #8a5300;">Open it</a></p>
  </div>
  {% endfor %}
  <p><a href="{{ notifications_url }}" style="display: inline-block; padding: 10px 18px; border-radius: 999px; background: #ffb454; color: #000; text-decoration: none;">See all notifications</a></p>
  <p style="color: #666; font-size: 13px;">You get this email when someone tags you with @ and you haven't opened it within 15 minutes.</p>
</body>
</html>
