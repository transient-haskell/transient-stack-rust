{-# LANGUAGE ScopedTypeVariables, OverloadedStrings #-}
import Transient.Internals
import Transient.Parse
import Control.Monad
import Control.Applicative
import Control.Exception hiding (try)
import Control.Monad.IO.Class
import qualified Data.ByteString.Lazy.Char8 as BS
import Data.Char
import Data.IORef
import Control.Monad.State (gets)

-- runs a parser on a string: the result and what is left unparsed
p :: Show a => String -> TransIO a -> String -> IO ()
p label parser input = do
  r <- (do
    (res, _) <- runTransient $ withParseString (BS.pack input) $ do
                  x <- parser
                  rest <- notParsed
                  return (x, BS.unpack rest)
    evaluate (length (show res)) >> return (show res)) `catch` \(e :: SomeException) -> return ("EXCEPCION " ++ takeWhile (/= '\n') (show e))
  putStrLn (label ++ "  " ++ show input ++ " => " ++ r)

-- the parser over a stream of chunks
ps :: Show a => String -> TransIO a -> [String] -> IO ()
ps label parser chunks = do
  ref <- newIORef chunks
  let more = liftIO $ atomicModifyIORef ref $ \cs -> case cs of
                [] -> ([], SDone)
                [c] -> ([], SLast (BS.pack c))
                (c:r) -> (r, SMore (BS.pack c))
  (res, _) <- runTransient $ withParseStream more $ do
                x <- parser
                b <- getParseBuffer
                return (x, BS.unpack b)
  putStrLn (label ++ "  " ++ show chunks ++ " => " ++ show res)

main = do
  p "serial: the first fails, the second does not start" (((,) <$> tChar 'x' <*> tChar 'a') <|> (notParsed >>= \r -> return ('-', BS.head r))) "ab"
  p "serial: both" ((,) <$> tChar 'a' <*> tChar 'b') "abc"
  p "serial: the second fails after the first consumed" (((,) <$> tChar 'a' <*> tChar 'x') <|> (notParsed >>= \r -> return ('-', BS.head r))) "ab"
  p "mode" (gets' execMode) "x"
  where gets' f = (transient $ do { m <- gets f; return (Just (show m)) })
