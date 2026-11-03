using System.Collections;
using System.Collections.Generic;
using System.Text;

/// <summary>Small deterministic JSON writer for the probe report.</summary>
public static class MiniJson
{
    public static string Serialize(object value)
    {
        var builder = new StringBuilder();
        Write(builder, value);
        return builder.ToString();
    }

    private static void Write(StringBuilder builder, object value)
    {
        switch (value)
        {
            case null: builder.Append("null"); break;
            case string s: builder.Append('"').Append(s.Replace("\\", "\\\\").Replace("\"", "\\\"")).Append('"'); break;
            case bool b: builder.Append(b ? "true" : "false"); break;
            case float f: builder.Append(f.ToString("R")); break;
            case double d: builder.Append(d.ToString("R")); break;
            case int i: builder.Append(i); break;
            case long l: builder.Append(l); break;
            case IDictionary<string, object> map:
                builder.Append('{');
                var first = true;
                foreach (var pair in map)
                {
                    if (!first) builder.Append(',');
                    first = false;
                    Write(builder, pair.Key);
                    builder.Append(':');
                    Write(builder, pair.Value);
                }
                builder.Append('}');
                break;
            case IEnumerable list:
                builder.Append('[');
                var firstItem = true;
                foreach (var item in list)
                {
                    if (!firstItem) builder.Append(',');
                    firstItem = false;
                    Write(builder, item);
                }
                builder.Append(']');
                break;
            default: builder.Append('"').Append(value).Append('"'); break;
        }
    }
}
